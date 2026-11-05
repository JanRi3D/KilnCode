#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod demo;
#[cfg(test)]
mod e2e;
mod git;
mod highlight;
mod icons;
mod log;
mod model;
mod provider;
mod settings;
mod store;
mod suggest;
mod terminal;
mod theme;
mod ui;
mod widgets;

use std::path::PathBuf;

struct KilnApp {
    app: app::App,
    shot: Option<(PathBuf, u32)>,
    frames: u32,
}

impl eframe::App for KilnApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        if ui.input(|i| i.viewport().close_requested()) {
            self.app.shutdown();
        }
        ui::root(&mut self.app, ui);
        self.screenshot_hook(ui.ctx());
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        theme::p().bg.to_normalized_gamma_f32()
    }

    fn on_exit(&mut self) {
        self.app.shutdown();
    }
}

impl KilnApp {
    /// `KILN_SCREENSHOT=out.png` saves a PNG of the window after it settles (used for validation).
    fn screenshot_hook(&mut self, ctx: &egui::Context) {
        let Some((path, after)) = self.shot.clone() else { return };
        self.frames += 1;
        if self.frames == after {
            let c = ctx.clone();
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::ScreenshotCallback::new(move |img| {
                let _ = std::fs::write(&path, png(&img));
                if std::env::var("KILN_EXIT_AFTER_SHOT").is_ok() {
                    c.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                c.request_repaint();
            })));
            self.shot = None;
        }
        if self.frames < after {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }
}

/// Minimal PNG encoder (stored deflate blocks) so screenshots need no image crate.
fn png(img: &egui::ColorImage) -> Vec<u8> {
    fn crc(data: &[u8]) -> u32 {
        let mut c = 0xffff_ffffu32;
        for &b in data {
            c ^= b as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xedb8_8320 ^ (c >> 1) } else { c >> 1 };
            }
        }
        !c
    }
    fn chunk(out: &mut Vec<u8>, kind: &[u8], data: &[u8]) {
        out.extend((data.len() as u32).to_be_bytes());
        let mut body = kind.to_vec();
        body.extend_from_slice(data);
        out.extend_from_slice(&body);
        out.extend(crc(&body).to_be_bytes());
    }
    let (w, h) = (img.size[0], img.size[1]);
    let mut raw = Vec::with_capacity((w * 4 + 1) * h);
    for y in 0..h {
        raw.push(0);
        for x in 0..w {
            let c = img.pixels[y * w + x];
            raw.extend_from_slice(&[c.r(), c.g(), c.b(), 255]);
        }
    }
    let mut z = vec![0x78, 0x01];
    let (mut a, mut b) = (1u32, 0u32);
    for &v in &raw {
        a = (a + v as u32) % 65521;
        b = (b + a) % 65521;
    }
    let mut chunks = raw.chunks(65535).peekable();
    while let Some(c) = chunks.next() {
        z.push(if chunks.peek().is_none() { 1 } else { 0 });
        z.extend((c.len() as u16).to_le_bytes());
        z.extend((!(c.len() as u16)).to_le_bytes());
        z.extend_from_slice(c);
    }
    z.extend(((b << 16) | a).to_be_bytes());
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut ihdr = Vec::new();
    ihdr.extend((w as u32).to_be_bytes());
    ihdr.extend((h as u32).to_be_bytes());
    ihdr.extend_from_slice(&[8, 6, 0, 0, 0]);
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"IDAT", &z);
    chunk(&mut out, b"IEND", &[]);
    out
}

/// A small kiln-arch icon drawn procedurally (no image assets needed).
fn app_icon() -> egui::IconData {
    let n = 64usize;
    let mut rgba = vec![0u8; n * n * 4];
    for y in 0..n {
        for x in 0..n {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let inside_rr = {
                let r = 14.0;
                let cx = fx.clamp(r, n as f32 - r);
                let cy = fy.clamp(r, n as f32 - r);
                (fx - cx).powi(2) + (fy - cy).powi(2) <= r * r
            };
            if !inside_rr {
                continue;
            }
            let mut c = [10u8, 10, 10, 255];
            // Arch: outer radius 20, inner 11, centered at (32, 38), above y=46.
            let d = ((fx - 32.0).powi(2) + (fy - 38.0).powi(2)).sqrt();
            let arch = fy <= 38.0 && (11.0..=20.0).contains(&d)
                || (fy > 38.0 && fy <= 48.0 && ((12.0..=21.0).contains(&fx) || (43.0..=52.0).contains(&fx)));
            if arch {
                c = [52, 107, 241, 255];
            }
            let flame = ((fx - 32.0) / 5.0).powi(2) + ((fy - 41.0) / 7.0).powi(2) <= 1.0;
            if flame {
                c = [255, 185, 0, 255];
            }
            rgba[(y * n + x) * 4..(y * n + x) * 4 + 4].copy_from_slice(&c);
        }
    }
    egui::IconData { rgba, width: n as u32, height: n as u32 }
}

fn main() -> eframe::Result<()> {
    let demo = std::env::args().any(|a| a == "--demo");
    if demo {
        let dir = std::env::temp_dir().join(format!("kiln-demo-{}", std::process::id()));
        // Always start from fresh sample data (PIDs get reused).
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::create_dir_all(&dir);
        // SAFETY: set before any other thread starts.
        unsafe { std::env::set_var("KILN_DATA_DIR", &dir) };
    }
    let data = log::data_dir();
    let _ = std::fs::create_dir_all(&data);
    log::init();
    log::info(&format!("Kiln Code {} starting (demo: {demo})", env!("CARGO_PKG_VERSION")));
    let store = match store::Store::open(&data.join("kiln.db")) {
        Ok(s) => s,
        Err(e) => {
            log::error(&format!("cannot open database: {e}"));
            eprintln!("Kiln Code couldn't open its database at {}: {e}", data.join("kiln.db").display());
            std::process::exit(1);
        }
    };
    if demo {
        demo::seed(&store);
    }
    let shot = std::env::var("KILN_SCREENSHOT")
        .ok()
        .map(|p| (PathBuf::from(p), std::env::var("KILN_SHOT_FRAMES").ok().and_then(|f| f.parse().ok()).unwrap_or(25)));
    let size = std::env::var("KILN_WINDOW")
        .ok()
        .and_then(|s| s.split_once('x').and_then(|(a, b)| Some((a.parse::<f32>().ok()?, b.parse::<f32>().ok()?))))
        .unwrap_or((1440.0, 900.0));
    let options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        viewport: egui::ViewportBuilder::default()
            .with_title("Kiln Code")
            .with_inner_size([size.0, size.1])
            .with_min_inner_size([720.0, 480.0])
            .with_icon(app_icon()),
        persist_window: shot.is_none(),
        persistence_path: Some(data.join("window.ron")),
        ..Default::default()
    };
    eframe::run_native(
        "Kiln Code",
        options,
        Box::new(move |cc| {
            egui_extras::install_image_loaders(&cc.egui_ctx);
            let settings = store.load_settings();
            theme::install_fonts(&cc.egui_ctx, &settings.ui_font, &settings.mono_font);
            icons::install(&cc.egui_ctx);
            cc.egui_ctx.options_mut(|o| o.zoom_with_keyboard = true);
            let mut app = app::App::new(cc.egui_ctx.clone(), store, demo);
            if demo {
                demo::attach_runtime(&mut app);
            }
            if let Ok(v) = std::env::var("KILN_VIEW") {
                demo::apply_view(&mut app, &v);
            }
            Ok(Box::new(KilnApp { app, shot, frames: 0 }))
        }),
    )
}
