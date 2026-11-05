//! PTY-backed interactive terminal for egui.
//!
//! * `portable-pty` spawns the shell (ConPTY on Windows).
//! * A reader thread feeds PTY output into a `vt100::Parser`; a writer thread
//!   drains keyboard input; a waiter thread notices when the child exits.
//! * [`terminal_view`] paints the vt100 cell grid with the egui painter and
//!   translates egui input into terminal bytes.

// Public API used as `mod terminal;` in a binary: not every method has a caller.
#![allow(dead_code)]

use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::num::NonZeroU16;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use egui::text::LayoutJob;
use egui::{
    Color32, Event, EventFilter, FontId, Galley, Key, Modifiers, MouseWheelUnit, PointerButton, Pos2, Rect, Sense, Stroke, StrokeKind,
    TextFormat, Vec2, pos2, vec2,
};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

/// Lines of scrollback kept per session.
const SCROLLBACK: usize = 5000;

/// Colors used to render a terminal. `ansi` is the 16-color palette
/// (0-7 normal, 8-15 bright); 256-color and truecolor are derived/passed through.
#[derive(Clone, Debug)]
pub struct TerminalColors {
    pub background: Color32,
    pub foreground: Color32,
    pub cursor: Color32,
    pub selection: Color32,
    pub ansi: [Color32; 16],
}

impl Default for TerminalColors {
    fn default() -> Self {
        let c = Color32::from_rgb;
        Self {
            background: c(0x00, 0x00, 0x00),
            foreground: c(0xd4, 0xd4, 0xd4),
            cursor: c(0xf5, 0xf5, 0xf5),
            selection: Color32::from_rgba_unmultiplied(81, 162, 255, 89),
            // VS Code's integrated terminal palette.
            ansi: [
                c(0x00, 0x00, 0x00),
                c(0xcd, 0x31, 0x31),
                c(0x0d, 0xbc, 0x79),
                c(0xe5, 0xe5, 0x10),
                c(0x24, 0x72, 0xc8),
                c(0xbc, 0x3f, 0xbc),
                c(0x11, 0xa8, 0xcd),
                c(0xe5, 0xe5, 0xe5),
                c(0x66, 0x66, 0x66),
                c(0xf1, 0x4c, 0x4c),
                c(0x23, 0xd1, 0x8b),
                c(0xf5, 0xf5, 0x43),
                c(0x3b, 0x8e, 0xea),
                c(0xd6, 0x70, 0xd6),
                c(0x29, 0xb8, 0xdb),
                c(0xe5, 0xe5, 0xe5),
            ],
        }
    }
}

/// vt100 callbacks: remembers the OSC title and queues replies to terminal
/// queries (the reader thread sends them back to the PTY).
#[derive(Default)]
struct Callbacks {
    title: String,
    reply: Vec<u8>,
}

impl vt100::Callbacks for Callbacks {
    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        self.title = String::from_utf8_lossy(title).into_owned();
    }

    fn unhandled_csi(&mut self, screen: &mut vt100::Screen, i1: Option<u8>, _i2: Option<u8>, params: &[&[u16]], c: char) {
        if i1.is_some() {
            return;
        }
        let first = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
        match (c, first) {
            // Cursor position report. ConPTY asks for this at startup and
            // waits for the answer, so it must be implemented.
            ('n', 6) => {
                let (row, col) = screen.cursor_position();
                self.reply.extend(format!("\x1b[{};{}R", row + 1, col + 1).into_bytes());
            }
            ('n', 5) => self.reply.extend(b"\x1b[0n"),
            // Primary device attributes: "VT100 with advanced video option".
            ('c', 0) => self.reply.extend(b"\x1b[?1;2c"),
            _ => {}
        }
    }
}

type Parser = vt100::Parser<Callbacks>;
/// A cell position: (row relative to the live screen top — negative rows are
/// in scrollback, col).
type GridPos = (i32, u16);

struct ChildState {
    child: Box<dyn Child + Send + Sync>,
    exit: Option<String>,
}

impl ChildState {
    /// Records the exit status if the child has exited. Returns `true` while alive.
    fn poll(&mut self) -> bool {
        if self.exit.is_none()
            && let Ok(Some(status)) = self.child.try_wait()
        {
            self.exit = Some(exit_message(&status));
        }
        self.exit.is_none()
    }
}

fn exit_message(status: &portable_pty::ExitStatus) -> String {
    match status.signal() {
        Some(sig) => format!("[process exited: {sig}]"),
        None => format!("[process exited with code {}]", status.exit_code()),
    }
}

/// Lock that survives a panicked holder (a dead reader thread must not take the UI down).
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn io_err(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// One shell running in a pseudo terminal.
pub struct TerminalSession {
    id: u64,
    cwd: PathBuf,
    shell_name: String,
    parser: Arc<Mutex<Parser>>,
    child: Arc<Mutex<ChildState>>,
    master: Option<Box<dyn MasterPty + Send>>,
    input: Option<Sender<Vec<u8>>>,
    threads: Vec<JoinHandle<()>>,
    size: (u16, u16),
    selection: Option<(GridPos, GridPos)>,
    scroll_accum: f32,
}

impl TerminalSession {
    /// Spawns `shell` (or the platform default shell) in `cwd`.
    ///
    /// `shell` may be a program path (spaces allowed if the file exists) or a
    /// full command line such as `cmd.exe /c echo hi` / `sh -c 'echo hi'`
    /// (split on whitespace, honoring single and double quotes).
    pub fn spawn(cwd: &Path, shell: Option<&str>, ctx: egui::Context) -> io::Result<Self> {
        let argv = match shell {
            Some(s) if Path::new(s.trim()).exists() => vec![s.trim().to_owned()],
            Some(s) => split_command_line(s),
            None => default_shell(),
        };
        let Some(program) = argv.first() else {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty shell command"));
        };
        let shell_name = Path::new(program).file_stem().map_or_else(|| program.clone(), |s| s.to_string_lossy().into_owned());

        let (rows, cols) = (24, 80);
        let pair = native_pty_system().openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }).map_err(io_err)?;
        // Take both ends before spawning so a failure here can't orphan a child.
        let mut reader = pair.master.try_clone_reader().map_err(io_err)?;
        let mut writer = pair.master.take_writer().map_err(io_err)?;

        let mut cmd = CommandBuilder::from_argv(argv.iter().map(OsString::from).collect());
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        let child = pair.slave.spawn_command(cmd).map_err(io_err)?;
        // Our copy of the slave must go, otherwise (on Unix) the reader never sees EOF.
        drop(pair.slave);

        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let parser = Arc::new(Mutex::new(vt100::Parser::new_with_callbacks(
            NonZeroU16::new(rows).unwrap_or(NonZeroU16::MIN),
            NonZeroU16::new(cols).unwrap_or(NonZeroU16::MIN),
            SCROLLBACK,
            Callbacks::default(),
        )));
        let child = Arc::new(Mutex::new(ChildState { child, exit: None }));

        // Construct the session first: if a thread fails to spawn, Drop kills the child.
        let mut session = Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            cwd: cwd.to_path_buf(),
            shell_name,
            parser: parser.clone(),
            child: child.clone(),
            master: Some(pair.master),
            input: Some(tx.clone()),
            threads: Vec::new(),
            size: (rows, cols),
            selection: None,
            scroll_accum: 0.0,
        };

        let repaint = ctx.clone();
        session.threads.push(thread::Builder::new().name("kiln-term-reader".into()).spawn(move || {
            let mut buf = vec![0u8; 64 * 1024];
            loop {
                let n = match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => n,
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                };
                let reply = {
                    let mut p = lock(&parser);
                    p.process(&buf[..n]);
                    std::mem::take(&mut p.callbacks_mut().reply)
                };
                if !reply.is_empty() {
                    let _ = tx.send(reply);
                }
                repaint.request_repaint();
            }
            repaint.request_repaint();
        })?);

        session.threads.push(thread::Builder::new().name("kiln-term-writer".into()).spawn(move || {
            // Ends once every sender (session + reader thread) is gone.
            for bytes in rx {
                if writer.write_all(&bytes).and_then(|()| writer.flush()).is_err() {
                    break;
                }
            }
        })?);

        // ponytail: polls every 50ms because ConPTY gives no EOF on child exit
        // and `Child::wait` would hold the lock `kill()` needs.
        session.threads.push(thread::Builder::new().name("kiln-term-waiter".into()).spawn(move || {
            while lock(&child).poll() {
                thread::sleep(Duration::from_millis(50));
            }
            ctx.request_repaint();
        })?);

        Ok(session)
    }

    /// OSC window title if the shell/app set one, else the shell's name.
    pub fn title(&self) -> String {
        let p = lock(&self.parser);
        let title = p.callbacks().title.trim();
        if title.is_empty() { self.shell_name.clone() } else { title.to_owned() }
    }

    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub fn is_alive(&self) -> bool {
        lock(&self.child).poll()
    }

    /// `Some("[process exited with code N]")` once the child has exited.
    pub fn exit_status(&self) -> Option<String> {
        let mut st = lock(&self.child);
        st.poll();
        st.exit.clone()
    }

    /// Sends raw bytes to the shell (as if typed) and scrolls to the bottom.
    pub fn write_input(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        lock(&self.parser).screen_mut().set_scrollback(0);
        self.send(bytes.to_vec());
    }

    fn send(&self, bytes: Vec<u8>) {
        if let Some(tx) = &self.input {
            let _ = tx.send(bytes);
        }
    }

    /// Current grid size as (rows, cols).
    pub fn size(&self) -> (u16, u16) {
        self.size
    }

    /// Resizes the PTY and the parser. [`terminal_view`] calls this every frame.
    pub fn resize(&mut self, rows: u16, cols: u16) {
        let (rows, cols) = (rows.max(1), cols.max(1));
        if (rows, cols) == self.size {
            return;
        }
        self.size = (rows, cols);
        if let Some(master) = &self.master {
            let _ = master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 });
        }
        lock(&self.parser).set_size(NonZeroU16::new(rows).unwrap_or(NonZeroU16::MIN), NonZeroU16::new(cols).unwrap_or(NonZeroU16::MIN));
    }

    /// Kills and reaps the child, closes the PTY and stops the I/O threads.
    /// Safe to call more than once.
    pub fn kill(&mut self) {
        {
            let mut st = lock(&self.child);
            if st.poll() {
                let _ = st.child.kill();
                st.exit = Some(match st.child.wait() {
                    Ok(status) => exit_message(&status),
                    Err(_) => "[process killed]".to_owned(),
                });
            }
        }
        self.input = None;
        // Dropping the master closes the pseudo console, so the reader hits EOF.
        self.master = None;
        for t in self.threads.drain(..) {
            join_timeout(t, Duration::from_millis(500));
        }
    }

    /// Visible screen contents as plain text.
    pub fn screen_text(&self) -> String {
        lock(&self.parser).screen().contents()
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        self.kill();
    }
}

/// Joins `t` unless it is still stuck after `timeout` (then it is detached;
/// it exits by itself once its pipe closes).
fn join_timeout(t: JoinHandle<()>, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while !t.is_finished() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(2));
    }
    if t.is_finished() {
        let _ = t.join();
    }
}

fn default_shell() -> Vec<String> {
    #[cfg(windows)]
    {
        let on_path = |exe: &str| {
            std::env::var_os("PATH").is_some_and(|p| {
                // symlink_metadata: WindowsApps aliases are reparse points.
                std::env::split_paths(&p).any(|d| std::fs::symlink_metadata(d.join(exe)).is_ok())
            })
        };
        let exe = if on_path("pwsh.exe") { "pwsh.exe" } else { "powershell.exe" };
        vec![exe.to_owned(), "-NoLogo".to_owned()]
    }
    #[cfg(not(windows))]
    {
        let shell = std::env::var("SHELL").ok().filter(|s| !s.is_empty());
        vec![shell.unwrap_or_else(|| "/bin/sh".to_owned())]
    }
}

/// Whitespace split honoring '…' and "…" quoting (no backslash escapes, so
/// Windows paths survive).
fn split_command_line(s: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut quote = None;
    let mut in_arg = false;
    for c in s.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None if c == '"' || c == '\'' => {
                quote = Some(c);
                in_arg = true;
            }
            None if c.is_whitespace() => {
                if in_arg {
                    args.push(std::mem::take(&mut cur));
                    in_arg = false;
                }
            }
            None => {
                cur.push(c);
                in_arg = true;
            }
        }
    }
    if in_arg {
        args.push(cur);
    }
    args
}

/// Maps a non-text key press to the bytes an xterm would send.
/// Printable characters are not handled here; they arrive as `Event::Text`.
pub fn key_to_bytes(key: Key, mods: Modifiers, app_cursor: bool) -> Option<Vec<u8>> {
    // xterm modifier parameter: 1 + shift + 2*alt + 4*ctrl.
    let m = 1 + u8::from(mods.shift) + 2 * u8::from(mods.alt) + 4 * u8::from(mods.ctrl);
    let cursor = |f: char| match (m, app_cursor) {
        (1, true) => format!("\x1bO{f}"),
        (1, false) => format!("\x1b[{f}"),
        _ => format!("\x1b[1;{m}{f}"),
    };
    let tilde = |n: u8| {
        if m == 1 { format!("\x1b[{n}~") } else { format!("\x1b[{n};{m}~") }
    };
    let ss3 = |f: char| {
        if m == 1 { format!("\x1bO{f}") } else { format!("\x1b[1;{m}{f}") }
    };
    let alt = |b: &[u8]| {
        if mods.alt { [b"\x1b", b].concat() } else { b.to_vec() }
    };

    let s = match key {
        Key::Enter => return Some(alt(b"\r")),
        Key::Backspace => return Some(alt(if mods.ctrl { b"\x08" } else { b"\x7f" })),
        Key::Tab if mods.shift => "\x1b[Z".to_owned(),
        Key::Tab => "\t".to_owned(),
        Key::Escape => "\x1b".to_owned(),
        Key::ArrowUp => cursor('A'),
        Key::ArrowDown => cursor('B'),
        Key::ArrowRight => cursor('C'),
        Key::ArrowLeft => cursor('D'),
        Key::Home => cursor('H'),
        Key::End => cursor('F'),
        Key::Insert => tilde(2),
        Key::Delete => tilde(3),
        Key::PageUp => tilde(5),
        Key::PageDown => tilde(6),
        Key::F1 => ss3('P'),
        Key::F2 => ss3('Q'),
        Key::F3 => ss3('R'),
        Key::F4 => ss3('S'),
        Key::F5 => tilde(15),
        Key::F6 => tilde(17),
        Key::F7 => tilde(18),
        Key::F8 => tilde(19),
        Key::F9 => tilde(20),
        Key::F10 => tilde(21),
        Key::F11 => tilde(23),
        Key::F12 => tilde(24),
        // Ctrl+Alt is AltGr on Windows layouts; that text comes as Event::Text.
        _ if mods.ctrl && !mods.alt => {
            let b = match key {
                Key::Space => 0,
                Key::OpenBracket => 0x1b,
                Key::Backslash => 0x1c,
                Key::CloseBracket => 0x1d,
                _ => match key.name().as_bytes() {
                    [l @ b'A'..=b'Z'] => l & 0x1f,
                    _ => return None,
                },
            };
            return Some(vec![b]);
        }
        _ => return None,
    };
    Some(s.into_bytes())
}

fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    // Strip ESC so pasted text can't end bracketed paste or inject sequences.
    let text: String = text.replace("\r\n", "\r").replace('\n', "\r").replace('\x1b', "");
    if bracketed { format!("\x1b[200~{text}\x1b[201~").into_bytes() } else { text.into_bytes() }
}

fn resolve_color(c: vt100::Color, default: Color32, ansi: &[Color32; 16]) -> Color32 {
    match c {
        vt100::Color::Default => default,
        vt100::Color::Idx(i) if i < 16 => ansi[usize::from(i)],
        vt100::Color::Idx(i) if i < 232 => {
            let level = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            let i = i - 16;
            Color32::from_rgb(level(i / 36), level((i / 6) % 6), level(i % 6))
        }
        vt100::Color::Idx(i) => Color32::from_gray(8 + (i - 232) * 10),
        vt100::Color::Rgb(r, g, b) => Color32::from_rgb(r, g, b),
    }
}

/// (fg, bg) of a cell after bold-as-bright, dim and inverse.
fn cell_colors(cell: &vt100::Cell, colors: &TerminalColors) -> (Color32, Color32) {
    let fg = match cell.fgcolor() {
        vt100::Color::Idx(i) if cell.bold() && i < 8 => colors.ansi[usize::from(i) + 8],
        c => resolve_color(c, colors.foreground, &colors.ansi),
    };
    let bg = resolve_color(cell.bgcolor(), colors.background, &colors.ansi);
    let fg = if cell.dim() { fg.lerp_to_gamma(bg, 0.4) } else { fg };
    if cell.inverse() { (bg, fg) } else { (fg, bg) }
}

fn is_word_char(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| !c.is_whitespace() && !"()[]{}<>'\"`,;|".contains(c))
}

/// Inclusive column range of the word under (row, col) in view coordinates.
fn word_at(screen: &vt100::Screen, row: u16, col: u16) -> (u16, u16) {
    let cols = screen.size().1.get();
    let word = |c: u16| screen.cell(row, c).is_some_and(|cell| is_word_char(cell.contents()));
    if !word(col) {
        return (col, col);
    }
    let (mut a, mut b) = (col, col);
    while a > 0 && word(a - 1) {
        a -= 1;
    }
    while b + 1 < cols && word(b + 1) {
        b += 1;
    }
    (a, b)
}

/// Normalized selection in view rows, clamped to the visible screen:
/// (start_row, start_col, end_row, end_col_exclusive).
fn visible_selection(screen: &vt100::Screen, sel: (GridPos, GridPos)) -> Option<(u16, u16, u16, u16)> {
    let (rows, cols) = (i32::from(screen.size().0.get()), screen.size().1.get());
    let (a, b) = if sel.0 <= sel.1 { sel } else { (sel.1, sel.0) };
    let off = i32::try_from(screen.scrollback()).unwrap_or(i32::MAX);
    let (r0, r1) = (a.0.saturating_add(off), b.0.saturating_add(off));
    if r1 < 0 || r0 >= rows {
        return None;
    }
    let (r0, c0) = if r0 < 0 { (0, 0) } else { (r0 as u16, a.1) };
    let (r1, c1) = if r1 >= rows { ((rows - 1) as u16, cols) } else { (r1 as u16, b.1 + 1) };
    Some((r0, c0, r1, c1.min(cols)))
}

fn selection_text(screen: &vt100::Screen, sel: (GridPos, GridPos)) -> String {
    // ponytail: only the part of the selection currently on screen is copied.
    visible_selection(screen, sel).map(|(r0, c0, r1, c1)| screen.contents_between(r0, c0, r1, c1)).unwrap_or_default()
}

/// Draws the terminal filling `ui.available_size()`, handles input, resize and
/// selection. Click to focus; while focused, Tab/arrows/Esc go to the terminal.
pub fn terminal_view(ui: &mut egui::Ui, term: &mut TerminalSession, font: FontId, colors: &TerminalColors) -> egui::Response {
    let id = egui::Id::unique(("kiln_terminal", term.id));
    let (_, rect) = ui.allocate_space(ui.available_size());
    let response = ui.interact(rect, id, Sense::click_and_drag());
    let ctx = ui.ctx().clone();

    let (cell_w, row_h) = ctx.fonts_mut(|f| (f.glyph_width(&font, 'M'), f.row_height(&font)));
    // Whole physical pixels per row, so background rects don't leave seams.
    let ppp = ctx.pixels_per_point();
    let row_h = ((row_h * ppp).round() / ppp).max(1.0);
    // glyph_width is 0.0 when no font is installed (egui without default_fonts).
    let cell = vec2(if cell_w > 0.0 { cell_w } else { font.size * 0.6 }, row_h);
    // `as` saturates; resize() clamps to >= 1.
    term.resize((rect.height() / cell.y) as u16, (rect.width() / cell.x) as u16);

    if response.clicked() || response.drag_started() {
        response.request_focus();
    }
    let focused = response.has_focus();
    if focused {
        ctx.memory_mut(|m| {
            m.set_focus_lock_filter(id, EventFilter { tab: true, horizontal_arrows: true, vertical_arrows: true, escape: true });
        });
    }

    let (events, mods, press_origin) = ctx.input(|i| (i.events.clone(), i.modifiers, i.pointer.press_origin()));
    let mut out: Vec<u8> = Vec::new();
    let mut parser = lock(&term.parser);
    let (rows, cols) = (parser.screen().size().0.get(), parser.screen().size().1.get());
    let offset = i32::try_from(parser.screen().scrollback()).unwrap_or(i32::MAX);
    let view_cell = |p: Pos2| -> (u16, u16) {
        let col = ((p.x - rect.left()) / cell.x).floor().clamp(0.0, f32::from(cols - 1));
        let row = ((p.y - rect.top()) / cell.y).floor().clamp(0.0, f32::from(rows - 1));
        (row as u16, col as u16)
    };
    let live_cell = |p: Pos2| -> GridPos {
        let (r, c) = view_cell(p);
        (i32::from(r) - offset, c)
    };

    // Mouse selection.
    if response.double_clicked() {
        if let Some(p) = response.interact_pointer_pos() {
            let (r, c) = view_cell(p);
            let (a, b) = word_at(parser.screen(), r, c);
            let r = i32::from(r) - offset;
            term.selection = Some(((r, a), (r, b)));
        }
    } else if response.drag_started_by(PointerButton::Primary) {
        if let (Some(o), Some(p)) = (press_origin, response.interact_pointer_pos()) {
            term.selection = Some((live_cell(o), live_cell(p)));
        }
    } else if response.dragged_by(PointerButton::Primary) {
        if let (Some(sel), Some(p)) = (&mut term.selection, response.interact_pointer_pos()) {
            sel.1 = live_cell(p);
        }
    } else if response.clicked() {
        term.selection = None;
    }

    let app_cursor = parser.screen().application_cursor();

    // Mouse wheel: scrollback, or arrow keys in the alternate screen (less, vim).
    if response.hovered() {
        for ev in &events {
            if let Event::MouseWheel { unit, delta, modifiers, .. } = ev
                && !modifiers.ctrl
                && !modifiers.command
            {
                term.scroll_accum += match unit {
                    MouseWheelUnit::Line => delta.y * 3.0,
                    MouseWheelUnit::Point => delta.y / cell.y,
                    MouseWheelUnit::Page => delta.y * f32::from(rows),
                };
            }
        }
        let lines = term.scroll_accum.trunc();
        term.scroll_accum -= lines;
        if lines != 0.0 {
            if parser.screen().alternate_screen() {
                let key = if lines > 0.0 { Key::ArrowUp } else { Key::ArrowDown };
                for _ in 0..(lines.abs() as usize) {
                    out.extend(key_to_bytes(key, Modifiers::NONE, app_cursor).unwrap_or_default());
                }
            } else {
                let target = (parser.screen().scrollback() as f32 + lines).max(0.0);
                parser.screen_mut().set_scrollback(target as usize);
            }
        }
    }

    // Keyboard.
    if focused {
        let bracketed = parser.screen().bracketed_paste();
        let mut copy = false;
        for ev in &events {
            match ev {
                Event::Text(t) => {
                    // Alt+key → ESC prefix (not on macOS, where Option composes characters).
                    if mods.alt && !mods.ctrl && !cfg!(target_os = "macos") {
                        out.push(0x1b);
                    }
                    out.extend(t.as_bytes());
                }
                Event::Paste(t) => out.extend(paste_bytes(t, bracketed)),
                // egui-winit turns Ctrl+C / Ctrl+X into Copy / Cut events.
                Event::Copy => {
                    if term.selection.is_some() {
                        copy = true;
                    } else if mods.ctrl {
                        out.push(0x03);
                    }
                }
                Event::Cut if mods.ctrl => out.push(0x18),
                Event::Key { key, pressed: true, modifiers, .. } => {
                    if modifiers.ctrl && modifiers.shift && *key == Key::C {
                        copy = true;
                    } else if modifiers.ctrl && modifiers.shift && *key == Key::V {
                        // egui-winit answers with an Event::Paste next frame.
                        ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
                    } else if let Some(b) = key_to_bytes(*key, *modifiers, app_cursor) {
                        out.extend(b);
                    }
                }
                _ => {}
            }
        }
        if copy && let Some(sel) = term.selection {
            let text = selection_text(parser.screen(), sel);
            if !text.is_empty() {
                ctx.copy_text(text);
            }
        }
    }
    if !out.is_empty() {
        parser.screen_mut().set_scrollback(0);
    }

    paint(ui.painter(), parser.screen(), rect, cell, &font, colors, focused, term.selection);
    drop(parser);
    if !out.is_empty() {
        term.send(out);
    }
    response
}

/// Consecutive cells drawn as one galley.
struct Run {
    col: u16,
    /// (fg, italic, underline)
    style: (Color32, bool, bool),
    text: String,
    /// A glyph that must sit alone on its cell (wide / odd-width / combining).
    solo: bool,
}

fn text_job(text: String, font: &FontId, fg: Color32, italic: bool, underline: bool) -> LayoutJob {
    let mut fmt = TextFormat::simple(font.clone(), fg);
    fmt.italics = italic;
    if underline {
        fmt.underline = Stroke::new(1.0, fg);
    }
    LayoutJob::single_section(text, fmt)
}

#[allow(clippy::too_many_arguments)]
fn paint(
    painter: &egui::Painter,
    screen: &vt100::Screen,
    rect: Rect,
    cell: Vec2,
    font: &FontId,
    colors: &TerminalColors,
    focused: bool,
    selection: Option<(GridPos, GridPos)>,
) {
    let (rows, cols) = (screen.size().0.get(), screen.size().1.get());
    let cell_pos = |row: u16, col: u16| rect.min + vec2(f32::from(col) * cell.x, f32::from(row) * cell.y);
    let cell_rect = |row: u16, col: u16, width: u16| Rect::from_min_size(cell_pos(row, col), vec2(f32::from(width) * cell.x, cell.y));

    // Cursor, in view coordinates.
    let (cur_row, cur_col) = screen.cursor_position();
    let cur_row = usize::from(cur_row) + screen.scrollback();
    let cursor = (!screen.hide_cursor() && cur_row < usize::from(rows)).then(|| (cur_row as u16, cur_col.min(cols - 1)));
    let block_cursor = focused
        && !matches!(
            screen.cursor_style(),
            vt100::CursorStyle::BlinkVerticalBar
                | vt100::CursorStyle::SteadyVerticalBar
                | vt100::CursorStyle::BlinkUnderline
                | vt100::CursorStyle::SteadyUnderline
        );

    let mut bgs: Vec<(Rect, Color32)> = Vec::new();
    let mut jobs: Vec<(Pos2, LayoutJob)> = Vec::new();
    let mut cursor_job: Option<(Pos2, LayoutJob)> = None;

    // All text layout under one fonts lock. Nothing in here may touch the
    // painter/context (egui's context lock is not reentrant).
    let (texts, cursor_glyph) = painter.fonts_mut(|f| {
        for row in 0..rows {
            // Background run: (start col, end col exclusive, color).
            let mut bg_run: Option<(u16, u16, Color32)> = None;
            let mut run: Option<Run> = None;
            let mut flush = |run: Option<Run>| {
                if let Some(Run { col, style: (fg, italic, underline), mut text, .. }) = run {
                    if !underline {
                        text.truncate(text.trim_end().len());
                    }
                    if !text.is_empty() {
                        let job = text_job(text, font, fg, italic, underline);
                        jobs.push((cell_pos(row, col), job));
                    }
                }
            };

            for col in 0..cols {
                let Some(c) = screen.cell(row, col) else {
                    break;
                };
                if c.is_wide_continuation() {
                    continue;
                }
                let width = if c.is_wide() { 2 } else { 1 };
                let (fg, bg) = cell_colors(c, colors);

                match &mut bg_run {
                    Some((_, end, color)) if *color == bg && *end == col => *end += width,
                    _ => {
                        if let Some((s, e, color)) = bg_run.take()
                            && color != colors.background
                        {
                            bgs.push((cell_rect(row, s, e - s), color));
                        }
                        bg_run = Some((col, col + width, bg));
                    }
                }

                let s = c.contents();
                if cursor == Some((row, col)) && block_cursor && !s.trim().is_empty() {
                    let job = text_job(s.to_owned(), font, colors.background, c.italic(), false);
                    cursor_job = Some((cell_pos(row, col), job));
                }
                // Glyphs whose advance isn't exactly one cell get their own galley,
                // so they can't shift the rest of the run off the grid.
                let mut chars = s.chars();
                let solo = width == 2
                    || match (chars.next(), chars.next()) {
                        (Some(ch), None) => !ch.is_ascii() && (f.glyph_width(font, ch) - cell.x).abs() > 0.5,
                        (None, _) => false,
                        _ => true, // combining sequences
                    };
                let style = (fg, c.italic(), c.underline());
                if !run.as_ref().is_some_and(|r| r.style == style && !r.solo && !solo) {
                    flush(run.take());
                    run = Some(Run { col, style, text: String::new(), solo });
                }
                if let Some(r) = &mut run {
                    r.text.push_str(if s.is_empty() { " " } else { s });
                }
            }
            flush(run);
            if let Some((s, e, color)) = bg_run
                && color != colors.background
            {
                bgs.push((cell_rect(row, s, e - s), color));
            }
        }
        let texts: Vec<(Pos2, Arc<Galley>)> = jobs.into_iter().map(|(p, job)| (p, f.layout_job(job))).collect();
        (texts, cursor_job.map(|(p, job)| (p, f.layout_job(job))))
    });

    painter.rect_filled(rect, 0.0, colors.background);
    for (r, color) in bgs {
        painter.rect_filled(r, 0.0, color);
    }
    if let Some((r0, c0, r1, c1)) = selection.and_then(|s| visible_selection(screen, s)) {
        for row in r0..=r1 {
            let start = if row == r0 { c0 } else { 0 };
            let end = if row == r1 { c1 } else { cols };
            if end > start {
                painter.rect_filled(cell_rect(row, start, end - start), 0.0, colors.selection);
            }
        }
    }
    if let Some((row, col)) = cursor
        && block_cursor
    {
        painter.rect_filled(cell_rect(row, col, 1), 0.0, colors.cursor);
    }
    for (pos, galley) in texts {
        painter.galley(pos, galley, colors.foreground);
    }
    if let Some((row, col)) = cursor {
        let r = cell_rect(row, col, 1);
        if !focused {
            painter.rect_stroke(r, 0.0, Stroke::new(1.0, colors.cursor), StrokeKind::Inside);
        } else if block_cursor {
            if let Some((pos, galley)) = cursor_glyph {
                painter.galley(pos, galley, colors.background);
            }
        } else if matches!(screen.cursor_style(), vt100::CursorStyle::BlinkUnderline | vt100::CursorStyle::SteadyUnderline) {
            painter.rect_filled(Rect::from_min_max(pos2(r.left(), r.bottom() - 2.0), r.max), 0.0, colors.cursor);
        } else {
            painter.rect_filled(Rect::from_min_size(r.min, vec2(2.0, r.height())), 0.0, colors.cursor);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_for(term: &TerminalSession, secs: u64, mut pred: impl FnMut(&str) -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < deadline {
            if pred(&term.screen_text()) {
                return true;
            }
            thread::sleep(Duration::from_millis(50));
        }
        false
    }

    fn interactive_shell() -> Option<&'static str> {
        if cfg!(windows) { Some("cmd.exe") } else { Some("sh") }
    }

    #[test]
    fn runs_command_line() {
        let cmd = if cfg!(windows) { "cmd.exe /c echo kiln-term-ok" } else { "sh -c 'echo kiln-term-ok'" };
        let dir = std::env::temp_dir();
        let term = TerminalSession::spawn(&dir, Some(cmd), egui::Context::default()).unwrap();
        assert_eq!(term.cwd(), dir.as_path());
        assert!(wait_for(&term, 10, |s| s.contains("kiln-term-ok")), "screen: {:?}", term.screen_text());
        let deadline = Instant::now() + Duration::from_secs(10);
        while term.is_alive() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(50));
        }
        assert_eq!(term.exit_status().as_deref(), Some("[process exited with code 0]"));
    }

    #[test]
    fn input_reaches_default_shell() {
        let term = TerminalSession::spawn(&std::env::temp_dir(), None, egui::Context::default()).unwrap();
        term.write_input(b"echo abc123\r");
        // The echoed command line also contains "abc123"; wait for the output line.
        assert!(wait_for(&term, 20, |s| s.lines().any(|l| l.trim() == "abc123")), "screen: {:?}", term.screen_text());
        assert!(term.is_alive());
    }

    #[test]
    fn kill_stops_child() {
        let mut term = TerminalSession::spawn(&std::env::temp_dir(), interactive_shell(), egui::Context::default()).unwrap();
        assert!(term.is_alive());
        term.kill();
        assert!(!term.is_alive());
        assert!(term.exit_status().is_some());
        assert!(term.threads.is_empty());
        term.kill(); // idempotent
    }

    #[test]
    fn key_mapping() {
        let none = Modifiers::NONE;
        let k = |key, mods, app| key_to_bytes(key, mods, app);
        assert_eq!(k(Key::Enter, none, false).unwrap(), b"\r");
        assert_eq!(k(Key::Backspace, none, false).unwrap(), b"\x7f");
        assert_eq!(k(Key::Tab, none, false).unwrap(), b"\t");
        assert_eq!(k(Key::Tab, Modifiers::SHIFT, false).unwrap(), b"\x1b[Z");
        assert_eq!(k(Key::Escape, none, false).unwrap(), b"\x1b");
        assert_eq!(k(Key::ArrowUp, none, false).unwrap(), b"\x1b[A");
        assert_eq!(k(Key::ArrowUp, none, true).unwrap(), b"\x1bOA");
        assert_eq!(k(Key::ArrowLeft, Modifiers::CTRL, false).unwrap(), b"\x1b[1;5D");
        assert_eq!(k(Key::Home, none, false).unwrap(), b"\x1b[H");
        assert_eq!(k(Key::End, none, true).unwrap(), b"\x1bOF");
        assert_eq!(k(Key::PageUp, none, false).unwrap(), b"\x1b[5~");
        assert_eq!(k(Key::Delete, Modifiers::SHIFT, false).unwrap(), b"\x1b[3;2~");
        assert_eq!(k(Key::Insert, none, false).unwrap(), b"\x1b[2~");
        assert_eq!(k(Key::F1, none, false).unwrap(), b"\x1bOP");
        assert_eq!(k(Key::F5, none, false).unwrap(), b"\x1b[15~");
        assert_eq!(k(Key::F12, none, false).unwrap(), b"\x1b[24~");
        assert_eq!(k(Key::C, Modifiers::CTRL, false).unwrap(), b"\x03");
        assert_eq!(k(Key::A, Modifiers::CTRL, false).unwrap(), b"\x01");
        assert_eq!(k(Key::Z, Modifiers::CTRL, false).unwrap(), b"\x1a");
        assert_eq!(k(Key::Space, Modifiers::CTRL, false).unwrap(), b"\0");
        assert_eq!(k(Key::Enter, Modifiers::ALT, false).unwrap(), b"\x1b\r");
        assert_eq!(k(Key::A, none, false), None); // printable: comes via Event::Text
        assert_eq!(k(Key::Q, Modifiers::CTRL | Modifiers::ALT, false), None); // AltGr
    }

    #[test]
    fn helpers() {
        assert_eq!(split_command_line(r#"sh -c 'echo hi there'"#), ["sh", "-c", "echo hi there"]);
        assert_eq!(split_command_line(r#""C:\Program Files\x.exe" -a"#), [r"C:\Program Files\x.exe", "-a"]);
        assert_eq!(paste_bytes("a\r\nb\x1b", true), b"\x1b[200~a\rb\x1b[201~");
        assert_eq!(paste_bytes("a\nb", false), b"a\rb");
        let ansi = TerminalColors::default().ansi;
        assert_eq!(resolve_color(vt100::Color::Idx(196), Color32::BLACK, &ansi), Color32::from_rgb(255, 0, 0));
        assert_eq!(resolve_color(vt100::Color::Idx(232), Color32::BLACK, &ansi), Color32::from_gray(8));
    }

    #[test]
    fn title_and_queries() {
        let mut p = vt100::Parser::new_with_callbacks(NonZeroU16::new(24).unwrap(), NonZeroU16::new(80).unwrap(), 0, Callbacks::default());
        p.process(b"\x1b]0;my title\x07ab\x1b[6n\x1b[c");
        assert_eq!(p.callbacks().title, "my title");
        assert_eq!(p.callbacks().reply, b"\x1b[1;3R\x1b[?1;2c");
    }

    fn frame(ctx: &egui::Context, term: &mut TerminalSession, events: Vec<Event>) {
        let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(800.0, 600.0))), events, ..Default::default() };
        ctx.run_ui(input, |ui| {
            terminal_view(ui, term, FontId::monospace(14.0), &TerminalColors::default());
        })
        .drop_without_applying_deltas();
    }

    #[test]
    fn view_renders_resizes_and_selects() {
        let ctx = egui::Context::default();
        let mut term = TerminalSession::spawn(&std::env::temp_dir(), interactive_shell(), ctx.clone()).unwrap();
        // Exercise the painter with every style (the shell may repaint over it later).
        lock(&term.parser).process(
            "\x1b[31mred\x1b[0m \x1b[1;7mbold-inv\x1b[0m \x1b[2;3;4m\x1b[38;5;208m256\x1b[38;2;1;2;3mrgb\x1b[0m 漢字 ─── ok\r\n".as_bytes(),
        );
        frame(&ctx, &mut term, vec![]);
        assert_ne!(term.size(), (24, 80), "terminal should resize to the 800x600 rect");

        term.write_input(b"echo hello-sel world\r");
        let is_output = |l: &str| l.trim() == "hello-sel world";
        assert!(wait_for(&term, 20, |s| s.lines().any(is_output)), "screen: {:?}", term.screen_text());
        let row = term.screen_text().lines().position(is_output).unwrap();

        // Double-click inside "hello-sel" selects the word.
        let font = FontId::monospace(14.0);
        let (cw, rh) = ctx.fonts_mut(|f| (f.glyph_width(&font, 'M'), f.row_height(&font)));
        let p = pos2(cw * 2.5, rh * (row as f32 + 0.5));
        let click = |pressed| Event::PointerButton { pos: p, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE };
        frame(&ctx, &mut term, vec![Event::PointerMoved(p), click(true), click(false)]);
        frame(&ctx, &mut term, vec![click(true), click(false)]);
        let sel = term.selection.expect("double click selects a word");
        assert_eq!(selection_text(lock(&term.parser).screen(), sel), "hello-sel");

        // Focused (clicked) + typed text reaches the shell.
        frame(&ctx, &mut term, vec![Event::Text("echo typed-ok".into())]);
        frame(
            &ctx,
            &mut term,
            vec![Event::Key { key: Key::Enter, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }],
        );
        assert!(wait_for(&term, 20, |s| s.lines().any(|l| l.trim() == "typed-ok")), "screen: {:?}", term.screen_text());
    }
}
