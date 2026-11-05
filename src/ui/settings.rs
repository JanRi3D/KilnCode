//! Settings overlay: searchable navigation, 22 sections, rows with descriptions, switches,
//! selectors and resets. Every control writes a real setting; features Kiln doesn't have
//! are explained instead of faked.

use crate::app::{App, Confirm};
use crate::icons as ic;
use crate::model::{ProviderKind, RuleDecision, now};
use crate::provider::{self, AuthState};
use crate::settings::{self as st, Settings};
use crate::theme::{self, fs, mono_fs, p, radius};
use crate::widgets::{self as w, Btn};
use egui::{Align, Align2, Color32, Frame, Layout, Margin, Rect, Sense, Stroke, Ui, pos2, vec2};

type Get<T> = fn(&Settings) -> T;
type Set<T> = fn(&mut Settings, T);

enum R {
    Toggle {
        t: &'static str,
        d: &'static str,
        get: Get<bool>,
        set: Set<bool>,
        def: bool,
        badge: Option<&'static str>,
    },
    Select {
        t: &'static str,
        d: &'static str,
        opts: &'static [&'static str],
        get: Get<String>,
        set: Set<String>,
        def: &'static str,
        w: f32,
    },
    Input {
        t: &'static str,
        d: &'static str,
        ph: &'static str,
        get: Get<String>,
        set: Set<String>,
        def: &'static str,
        w: f32,
        mono: bool,
    },
    Range {
        t: &'static str,
        d: &'static str,
        get: Get<u32>,
        set: Set<u32>,
        def: u32,
        min: u32,
        max: u32,
        step: u32,
        unit: &'static str,
    },
    Days {
        t: &'static str,
        d: &'static str,
        on: Get<bool>,
        set_on: Set<bool>,
        n: Get<u32>,
        set_n: Set<u32>,
        def_on: bool,
        def_n: u32,
    },
    Info(&'static str),
    Custom {
        t: &'static str,
        keys: &'static str,
        f: fn(&mut App, &mut Ui),
    },
}

impl R {
    fn text(&self) -> String {
        match self {
            R::Toggle { t, d, .. } | R::Select { t, d, .. } | R::Input { t, d, .. } | R::Range { t, d, .. } | R::Days { t, d, .. } => {
                format!("{t} {d}")
            }
            R::Info(s) => s.to_string(),
            R::Custom { t, keys, .. } => format!("{t} {keys}"),
        }
    }
}

struct Sec {
    title: &'static str,
    rows: Vec<R>,
    plain: bool,
}

fn sec(title: &'static str, rows: Vec<R>) -> Sec {
    Sec { title, rows, plain: false }
}
fn plain(title: &'static str, rows: Vec<R>) -> Sec {
    Sec { title, rows, plain: true }
}
fn tog(t: &'static str, d: &'static str, get: Get<bool>, set: Set<bool>, def: bool) -> R {
    R::Toggle { t, d, get, set, def, badge: None }
}
fn sel(
    t: &'static str,
    d: &'static str,
    opts: &'static [&'static str],
    get: Get<String>,
    set: Set<String>,
    def: &'static str,
    w: f32,
) -> R {
    R::Select { t, d, opts, get, set, def, w }
}
fn custom(t: &'static str, keys: &'static str, f: fn(&mut App, &mut Ui)) -> R {
    R::Custom { t, keys, f }
}

const GROUPS: &[(&str, &[&str])] = &[
    ("", &["general", "appearance", "chat", "diffs", "keybindings", "snapshots"]),
    ("Agents", &["providers", "approvals", "threads", "browser", "devices", "automations"]),
    ("Code", &["projects", "git", "worktrees"]),
    ("Connections", &["environments", "sharing"]),
    ("", &["usage", "maintenance", "labs", "about", "archive"]),
];

fn page_meta(id: &str) -> (&'static str, &'static str) {
    match id {
        "general" => ("General", ic::NAV_GENERAL),
        "appearance" => ("Appearance", ic::NAV_APPEARANCE),
        "chat" => ("Chat", ic::NAV_CHAT),
        "diffs" => ("Diffs & pull requests", ic::FILE_DIFF),
        "keybindings" => ("Keybindings", ic::NAV_KEYS),
        "snapshots" => ("SnapShots", ic::NAV_SNAPSHOTS),
        "providers" => ("Providers", ic::BOT),
        "approvals" => ("Approvals", ic::LIST_CHECKS),
        "threads" => ("Threads", ic::NAV_THREADS),
        "browser" => ("Browser", ic::GLOBE),
        "devices" => ("Devices", ic::NAV_DEVICES),
        "automations" => ("Scheduled tasks", ic::NAV_SCHEDULE),
        "projects" => ("Projects", ic::NAV_PROJECTS),
        "git" => ("Source control", ic::GIT_BRANCH),
        "worktrees" => ("Worktrees", ic::FOLDER_TREE),
        "environments" => ("Environments", ic::SERVER),
        "sharing" => ("Sharing & access", ic::SHIELD_CHECK),
        "usage" => ("Usage & limits", ic::NAV_USAGE),
        "maintenance" => ("Maintenance", ic::NAV_MAINT),
        "labs" => ("Labs", ic::NAV_LABS),
        "about" => ("About", ic::INFO),
        _ => ("Archive", ic::NAV_ARCHIVE),
    }
}

fn sections(id: &str) -> Vec<Sec> {
    match id {
        "general" => vec![
            sec(
                "Notifications",
                vec![
                    sel(
                        "Thread notifications",
                        "System alerts when a thread finishes, fails, or needs approval while Kiln is in the background.",
                        &["Off", "Notifications only", "Sound only", "Notifications with sound"],
                        |s| s.notifications.clone(),
                        |s, v| s.notifications = v,
                        "Notifications with sound",
                        224.0,
                    ),
                    tog(
                        "In-app notifications",
                        "Show a toast when another thread finishes, fails, or needs approval while Kiln has focus.",
                        |s| s.in_app_toasts,
                        |s, v| s.in_app_toasts = v,
                        true,
                    ),
                ],
            ),
            sec(
                "Sidebar",
                vec![
                    tog(
                        "Working section",
                        "Fold working threads into a Working section. They return to the top when they need you.",
                        |s| s.working_section,
                        |s, v| s.working_section = v,
                        true,
                    ),
                    R::Select {
                        t: "Sidebar density",
                        d: "Compact shows one line per thread, so more of them fit on screen.",
                        opts: &["Comfortable", "Compact"],
                        get: |s| s.sidebar_density.clone(),
                        set: |s, v| s.sidebar_density = v,
                        def: "Comfortable",
                        w: 160.0,
                    },
                    sel(
                        "Project order",
                        "Order of projects in project pickers and the board filter.",
                        &["Last user message", "Created at", "Name"],
                        |s| s.project_order.clone(),
                        |s, v| s.project_order = v,
                        "Last user message",
                        176.0,
                    ),
                ],
            ),
            sec(
                "Confirmations",
                vec![tog(
                    "Delete confirmation",
                    "Ask before deleting a thread and its chat history.",
                    |s| s.delete_confirm,
                    |s, v| s.delete_confirm = v,
                    true,
                )],
            ),
            sec(
                "Demo",
                vec![R::Info(
                    "Demo mode shows sample data for visual testing. Start Kiln with `kiln-code --demo`; it uses a separate, temporary database and never runs agents.",
                )],
            ),
        ],
        "appearance" => vec![
            plain(
                "",
                vec![
                    custom("Color scheme", "system light dark theme", scheme_tiles),
                    custom("Themes", "kiln ember forest violet accent", theme_tiles),
                ],
            ),
            sec(
                "Interface",
                vec![
                    R::Range {
                        t: "Contrast",
                        d: "At 50% and above, secondary text and soft borders use the strong colors.",
                        get: |s| s.contrast,
                        set: |s, v| s.contrast = v,
                        def: 0,
                        min: 0,
                        max: 100,
                        step: 5,
                        unit: "%",
                    },
                    sel(
                        "Chat width",
                        "How wide messages and the composer can grow on large screens.",
                        &["Comfortable", "Wide", "Full"],
                        |s| s.chat_width.clone(),
                        |s, v| s.chat_width = v,
                        "Comfortable",
                        160.0,
                    ),
                    sel(
                        "Effort display",
                        "Show reasoning effort in the composer as its name, such as High, or as bars.",
                        &["Word", "Bars"],
                        |s| s.effort_display.clone(),
                        |s, v| s.effort_display = v,
                        "Word",
                        160.0,
                    ),
                ],
            ),
            sec(
                "Typography",
                vec![
                    R::Input {
                        t: "Interface font",
                        d: "Path to a .ttf or .otf file. Leave empty for Segoe UI Variable (Windows) or the system default.",
                        ph: "System default",
                        get: |s| s.ui_font.clone(),
                        set: |s, v| s.ui_font = v,
                        def: "",
                        w: 260.0,
                        mono: false,
                    },
                    R::Range {
                        t: "Interface font size",
                        d: "Everything outside code blocks and the terminal.",
                        get: |s| s.ui_font_size,
                        set: |s, v| s.ui_font_size = v,
                        def: 14,
                        min: 12,
                        max: 16,
                        step: 1,
                        unit: " px",
                    },
                    R::Input {
                        t: "Monospace font",
                        d: "Path to a .ttf file for code, diffs and the terminal. Leave empty for Consolas or the system monospace font.",
                        ph: "System monospace",
                        get: |s| s.mono_font.clone(),
                        set: |s, v| s.mono_font = v,
                        def: "",
                        w: 260.0,
                        mono: true,
                    },
                    R::Range {
                        t: "Monospace font size",
                        d: "Code blocks, diffs, file previews and the terminal.",
                        get: |s| s.mono_font_size,
                        set: |s, v| s.mono_font_size = v,
                        def: 13,
                        min: 12,
                        max: 14,
                        step: 1,
                        unit: " px",
                    },
                    tog(
                        "Word wrap",
                        "Wrap long lines in code blocks and diffs by default.",
                        |s| s.word_wrap,
                        |s, v| s.word_wrap = v,
                        false,
                    ),
                ],
            ),
        ],
        "chat" => vec![
            sec(
                "Composer",
                vec![sel(
                    "Send shortcut",
                    "Enter sends and Shift+Enter adds a line, or Ctrl+Enter sends and Enter adds a line.",
                    &["Enter", "Ctrl + Enter"],
                    |s| s.send_shortcut.clone(),
                    |s, v| s.send_shortcut = v,
                    "Enter",
                    200.0,
                )],
            ),
            sec(
                "Follow-ups",
                vec![sel(
                    "Follow-up behavior",
                    "Queue follow-ups until the agent's turn ends, or steer the running turn. Steering needs provider support (Codex); Claude Code follow-ups are queued. Ctrl+Shift+Enter does the opposite for one message.",
                    &["Queue", "Steer"],
                    |s| s.follow_up.clone(),
                    |s, v| s.follow_up = v,
                    "Queue",
                    160.0,
                )],
            ),
        ],
        "diffs" => vec![
            sec(
                "Diffs",
                vec![
                    sel(
                        "Diff layout",
                        "Show diffs stacked or side by side. The toggle in the diff toolbar changes this for the session.",
                        &["Stacked", "Split"],
                        |s| s.diff_layout.clone(),
                        |s, v| s.diff_layout = v,
                        "Stacked",
                        160.0,
                    ),
                    tog(
                        "Hide whitespace changes",
                        "Hide added or removed lines that are only whitespace.",
                        |s| s.hide_whitespace,
                        |s, v| s.hide_whitespace = v,
                        false,
                    ),
                    sel(
                        "Diff colors",
                        "Colors for additions and deletions, including change counts.",
                        &["Red & green", "Blue & orange"],
                        |s| s.diff_colors.clone(),
                        |s, v| s.diff_colors = v,
                        "Red & green",
                        160.0,
                    ),
                    R::Toggle {
                        t: "Require every file to be viewed",
                        d: "Commit, push and Open pull request stay disabled until each changed file is marked Viewed. Viewed resets when a file changes again.",
                        get: |s| s.require_viewed,
                        set: |s, v| s.require_viewed = v,
                        def: false,
                        badge: None,
                    },
                ],
            ),
            sec(
                "Pull requests",
                vec![
                    tog(
                        "Open the changes panel proactively",
                        "After a turn that edits at least 3 files or 50 lines, open the diff panel.",
                        |s| s.proactive_changes,
                        |s, v| s.proactive_changes = v,
                        true,
                    ),
                    tog(
                        "Open pull requests as drafts",
                        "New pull requests start as drafts. You can change it before opening.",
                        |s| s.pr_draft_default,
                        |s, v| s.pr_draft_default = v,
                        true,
                    ),
                    R::Info("Pull requests are created with your installed GitHub CLI (`gh pr create`). Kiln never stores a GitHub token."),
                ],
            ),
        ],
        "keybindings" => vec![sec(
            "Keybindings",
            vec![custom("Keybindings", "shortcut keys palette new thread sidebar settings interrupt approve terminal", keybindings)],
        )],
        "snapshots" => vec![
            sec(
                "SnapShots",
                vec![R::Info(
                    "Window capture isn't available in this build: it would need OS screen-capture access that Kiln doesn't request. Attach screenshots with the paperclip in the composer instead.",
                )],
            ),
            sec(
                "Turn checkpoints",
                vec![
                    R::Toggle {
                        t: "Checkpoint before every agent turn",
                        d: "Snapshot tracked and untracked files (ignored files excluded) into a hidden git ref, so any turn can be diffed, restored or forked.",
                        get: |s| s.checkpoints,
                        set: |s, v| s.checkpoints = v,
                        def: true,
                        badge: None,
                    },
                    sel(
                        "Keep checkpoints for",
                        "Older checkpoint refs are deleted from the repository when Kiln starts.",
                        &["7 days", "30 days", "90 days"],
                        |s| format!("{} days", s.checkpoint_keep_days),
                        |s, v| s.checkpoint_keep_days = v.split(' ').next().and_then(|n| n.parse().ok()).unwrap_or(30),
                        "30 days",
                        160.0,
                    ),
                ],
            ),
        ],
        "providers" => vec![
            plain("", vec![custom("Providers", "claude code codex installed signed in version path model effort login", providers_panel)]),
            sec(
                "Advanced",
                vec![R::Range {
                    t: "Health check interval",
                    d: "Re-check provider versions and sign-in status in the background (read-only; never starts a login). 0 disables.",
                    get: |s| s.health_interval_secs,
                    set: |s, v| s.health_interval_secs = v,
                    def: 300,
                    min: 0,
                    max: 3600,
                    step: 60,
                    unit: " s",
                }],
            ),
        ],
        "approvals" => vec![
            sec(
                "Risk levels",
                vec![
                    sel(
                        "Read-only commands",
                        "Commands the provider marks as read, search or list (and Read/Grep tools). Kiln answers the provider's request automatically.",
                        &["Always allow", "Ask"],
                        |s| s.risk_read.clone(),
                        |s, v| s.risk_read = v,
                        "Always allow",
                        144.0,
                    ),
                    sel(
                        "Edits inside the worktree",
                        "File edits whose paths are all inside the thread's worktree or checkout.",
                        &["Always allow", "Ask"],
                        |s| s.risk_write.clone(),
                        |s, v| s.risk_write = v,
                        "Ask",
                        144.0,
                    ),
                    sel(
                        "Network access",
                        "Requests the provider flags as network access (web fetch, network approvals).",
                        &["Ask", "Block"],
                        |s| s.risk_network.clone(),
                        |s, v| s.risk_network = v,
                        "Ask",
                        144.0,
                    ),
                    sel(
                        "Outside the worktree",
                        "Requests that name paths outside the thread's workspace.",
                        &["Ask", "Block"],
                        |s| s.risk_outside.clone(),
                        |s, v| s.risk_outside = v,
                        "Ask",
                        144.0,
                    ),
                    R::Info(
                        "These policies only decide how Kiln answers permission requests the provider sends; the provider still enforces its own permission mode and sandbox.",
                    ),
                ],
            ),
            sec("Rules", vec![custom("Rules", "allow block always pattern command rule", rules_panel)]),
        ],
        "threads" => vec![
            sec(
                "New projects",
                vec![R::Input {
                    t: "Add project starts in",
                    d: "Folder the Add project picker opens in. Leave empty for your home folder.",
                    ph: "~/",
                    get: |s| s.add_project_start.clone(),
                    set: |s, v| s.add_project_start = v,
                    def: "",
                    w: 288.0,
                    mono: true,
                }],
            ),
            sec(
                "New threads",
                vec![
                    sel(
                        "Default provider",
                        "Provider preselected on the New thread screen.",
                        &["claude", "codex"],
                        |s| s.default_provider.clone(),
                        |s, v| s.default_provider = v,
                        "claude",
                        160.0,
                    ),
                    sel(
                        "Permissions",
                        "Supervised asks before commands and edits. Auto-accept edits lets file edits through. Plan is read-only. Full access skips approvals (Claude bypassPermissions; Codex danger-full-access) and is not recommended.",
                        &["Supervised", "Auto-accept edits", "Plan (read-only)", "Full access"],
                        |s| s.permissions.clone(),
                        |s, v| s.permissions = v,
                        "Supervised",
                        208.0,
                    ),
                    sel(
                        "Workspace",
                        "Where new threads start.",
                        &["New worktree", "Current checkout"],
                        |s| s.workspace.clone(),
                        |s, v| s.workspace = v,
                        "New worktree",
                        208.0,
                    ),
                    R::Range {
                        t: "Parallel agents",
                        d: "Threads beyond this wait in a queue until a slot frees up.",
                        get: |s| s.parallel_agents,
                        set: |s, v| s.parallel_agents = v,
                        def: 4,
                        min: 1,
                        max: 8,
                        step: 1,
                        unit: "",
                    },
                ],
            ),
            sec(
                "Auto-settle",
                vec![R::Days {
                    t: "Auto-settle inactive threads",
                    d: "Threads with no activity for this long move to Settled. New activity brings them back. Threads with a pull request stay.",
                    on: |s| s.auto_settle,
                    set_on: |s, v| s.auto_settle = v,
                    n: |s| s.auto_settle_days,
                    set_n: |s, v| s.auto_settle_days = v,
                    def_on: true,
                    def_n: 7,
                }],
            ),
        ],
        "browser" => {
            vec![sec("Browser", vec![R::Info("Kiln has no embedded browser or web preview. Links open in your default browser.")])]
        }
        "devices" => vec![sec(
            "Devices",
            vec![R::Info("Device and simulator control isn't available. Kiln runs agents on this computer only and has no phone pairing.")],
        )],
        "automations" => vec![sec(
            "Scheduled tasks",
            vec![R::Info(
                "Scheduled tasks aren't available: Kiln only runs agents while it's open and doesn't install a background service.",
            )],
        )],
        "projects" => vec![plain("", vec![custom("Projects", "project name path scripts actions pin archive remove", projects_panel)])],
        "git" => vec![
            sec(
                "Repositories",
                vec![sel(
                    "Default merge method",
                    "Used by Merge on the board and in Review & ship (`gh pr merge`).",
                    &["Squash and merge", "Merge", "Rebase and merge"],
                    |s| s.merge_method.clone(),
                    |s, v| s.merge_method = v,
                    "Squash and merge",
                    192.0,
                )],
            ),
            sec("Version control", vec![custom("Version control", "git github gh authenticated cli", vcs_panel)]),
        ],
        "worktrees" => vec![
            sec(
                "Creation",
                vec![
                    R::Input {
                        t: "Worktree location",
                        d: "Folder for new worktrees, such as D:\\worktrees or ~/worktrees. Existing worktrees stay where they are. Leave empty for ~/.kiln/worktrees.",
                        ph: "Default",
                        get: |s| s.worktree_location.clone(),
                        set: |s, v| s.worktree_location = v,
                        def: "",
                        w: 260.0,
                        mono: true,
                    },
                    tog(
                        "Start from remote",
                        "Fetch origin and branch from origin/<base> instead of the local base branch.",
                        |s| s.worktree_from_remote,
                        |s, v| s.worktree_from_remote = v,
                        false,
                    ),
                ],
            ),
            sec(
                "Branches",
                vec![
                    R::Input {
                        t: "Branch prefix",
                        d: "For example, kiln produces kiln/add-search. Leave empty for no prefix. Existing names get -2, -3…",
                        ph: "No prefix",
                        get: |s| s.branch_prefix.clone(),
                        set: |s, v| s.branch_prefix = v,
                        def: "kiln",
                        w: 224.0,
                        mono: true,
                    },
                    custom("Preview", "branch name preview", branch_preview),
                ],
            ),
            sec("Cleanup", vec![custom("Clean up worktrees", "cleanup remove delete disk worktrees", cleanup_panel)]),
        ],
        "environments" => vec![sec(
            "This app",
            vec![R::Info("Agents run on this computer. Remote environments (SSH hosts, dev containers) aren't available in this build.")],
        )],
        "sharing" => vec![sec(
            "Sharing & access",
            vec![
                R::Info(
                    "Kiln has no account, server, sync or remote access. Your data stays on this computer; only your providers' own traffic leaves it.",
                ),
                R::Info("Kiln's log files redact tokens and keys before writing them."),
            ],
        )],
        "usage" => vec![sec("Usage", vec![custom("Usage", "tokens cost estimate rate limits subscription", usage_panel)])],
        "maintenance" => vec![sec(
            "Logs",
            vec![
                R::Days {
                    t: "Delete old logs",
                    d: "Delete Kiln's daily log files after this many days. Today's log is kept.",
                    on: |s| s.log_retention_days > 0,
                    set_on: |s, v| s.log_retention_days = if v { 14 } else { 0 },
                    n: |s| s.log_retention_days,
                    set_n: |s, v| s.log_retention_days = v.max(1),
                    def_on: true,
                    def_n: 14,
                },
                custom("Files", "logs data folder database open delete", maintenance_panel),
            ],
        )],
        "labs" => vec![sec("Labs", vec![R::Info("There are no experimental features in this build.")])],
        "about" => vec![sec("App", vec![custom("About", "version build licenses data", about_panel)])],
        _ => vec![plain("", vec![custom("Archive", "archived threads unarchive restore", archive_panel)])],
    }
}

pub fn show(app: &mut App, ui: &mut Ui) {
    let pal = p();
    let ctx = ui.ctx().clone();
    let screen = ctx.content_rect();
    // Dim everything below the dialog; a click outside it closes the dialog.
    ctx.layer_painter(egui::LayerId::new(egui::Order::Middle, egui::Id::unique("settings_backdrop"))).rect_filled(
        screen,
        0.0,
        pal.backdrop,
    );
    egui::Area::new(egui::Id::unique("settings_backdrop_hit")).order(egui::Order::Middle).fixed_pos(screen.min).show(&ctx, |ui| {
        ui.set_min_size(screen.size());
        let resp = ui.interact(screen, ui.scope_id().with("hit"), Sense::click());
        if resp.clicked() {
            app.ui.settings_open = false;
        }
    });
    let narrow = screen.width() < 760.0;
    let pad = if narrow { 0.0 } else { 40.0 };
    let size = vec2((screen.width() - 2.0 * pad).min(1280.0), (screen.height() - 2.0 * pad).min(960.0));
    let pos = screen.center() - size / 2.0;
    egui::Area::new(egui::Id::unique("settings")).order(egui::Order::Foreground).fixed_pos(pos).show(&ctx, |ui| {
        let frame = Frame::new()
            .fill(pal.bg)
            .stroke(Stroke::new(if narrow { 0.0 } else { 1.0 }, pal.border))
            .corner_radius(radius(if narrow { 0 } else { 16 }))
            .shadow(egui::Shadow { offset: [0, 25], blur: 50, spread: 0, color: theme::rgba(0, 0, 0, 0.4) });
        frame.show(ui, |ui| {
            ui.set_min_size(size);
            ui.set_max_size(size);
            let full = ui.max_rect();
            let nav_w = if narrow { 56.0 } else { 240.0 };
            let nav = Rect::from_min_size(full.min, vec2(nav_w, full.height()));
            ui.painter().rect_filled(
                nav,
                egui::CornerRadius { nw: if narrow { 0 } else { 16 }, sw: if narrow { 0 } else { 16 }, ne: 0, se: 0 },
                pal.sidebar,
            );
            ui.painter().vline(nav.right(), nav.y_range(), Stroke::new(1.0, pal.border));
            let mut nui = ui.new_child(egui::UiBuilder::new().max_rect(nav.shrink(8.0)));
            nav_panel(app, &mut nui, narrow);
            let content = Rect::from_min_max(pos2(nav.right() + 1.0, full.top()), full.max);
            let mut cui = ui.new_child(egui::UiBuilder::new().max_rect(content));
            content_panel(app, &mut cui, narrow);
        });
    });
}

fn nav_panel(app: &mut App, ui: &mut Ui, narrow: bool) {
    let pal = p();
    egui::ScrollArea::vertical().id_salt("settings_nav").auto_shrink([false, false]).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        if !narrow {
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                w::icon(ui, ic::SEARCH, 16.0, pal.muted80);
                let r = ui.add(
                    egui::TextEdit::singleline(&mut app.ui.settings_query)
                        .hint_text("Search")
                        .frame(Frame::NONE)
                        .font(theme::ui_medium(fs(14.0)))
                        .desired_width(ui.available_width() - 32.0),
                );
                if app.ui.settings_query.is_empty() {
                    w::kbd(ui, "/");
                    if ui.input(|i| i.key_pressed(egui::Key::Slash)) && !r.has_focus() && ui.memory(|m| m.focused().is_none()) {
                        r.request_focus();
                    }
                } else if w::icon_button_sized(ui, ic::X, "Clear search", false, 20.0, 12.0, pal.muted).clicked() {
                    app.ui.settings_query.clear();
                }
            });
            ui.add_space(8.0);
        }
        for (label, pages) in GROUPS {
            if !label.is_empty() && !narrow {
                ui.horizontal(|ui| {
                    ui.add_space(8.0);
                    w::text_medium(ui, *label, 12.0, pal.muted80);
                });
            }
            for id in *pages {
                let (name, glyph) = page_meta(id);
                let on = app.ui.settings_query.is_empty() && app.ui.settings_page == *id;
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
                let bg = if on {
                    pal.row_sel
                } else if resp.hovered() {
                    pal.hover
                } else {
                    Color32::TRANSPARENT
                };
                ui.painter().rect_filled(rect, radius(theme::R_LG), bg);
                let fg = if on || resp.hovered() { pal.fg } else { pal.muted80 };
                let ic_x = if narrow { rect.center().x - 8.0 } else { rect.left() + 10.0 };
                w::paint_icon(
                    ui,
                    Rect::from_min_size(pos2(ic_x, rect.center().y - 8.0), vec2(16.0, 16.0)),
                    glyph,
                    16.0,
                    if on || resp.hovered() { pal.fg } else { pal.icon },
                );
                if !narrow {
                    ui.painter().text(pos2(rect.left() + 34.0, rect.center().y), Align2::LEFT_CENTER, name, theme::ui_medium(fs(14.0)), fg);
                }
                if resp.on_hover_text(name).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    app.ui.settings_page = id.to_string();
                    app.ui.settings_query.clear();
                }
            }
            ui.add_space(12.0);
        }
    });
}

fn content_panel(app: &mut App, ui: &mut Ui, narrow: bool) {
    let pal = p();
    let q = app.ui.settings_query.trim().to_lowercase();
    let page = app.ui.settings_page.clone();
    let title = if q.is_empty() { page_meta(&page).0 } else { "Search" };
    ui.allocate_ui_with_layout(vec2(ui.available_width(), 48.0), Layout::left_to_right(Align::Center), |ui| {
        ui.add_space(20.0);
        ui.spacing_mut().item_spacing.x = 12.0;
        w::text_medium(ui, "Settings", 14.0, pal.muted);
        w::text_medium(ui, "/", 14.0, pal.muted);
        w::text_medium(ui, title, 14.0, pal.fg);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(8.0);
            if w::icon_button_sized(ui, ic::X, "Close (Esc)", false, 28.0, 16.0, pal.fg).clicked() {
                app.ui.settings_open = false;
            }
            if page == "general" && q.is_empty() && Btn::ghost("Restore defaults").icon(ic::ROTATE_CCW).show(ui).clicked() {
                let d = Settings::default();
                let s = &mut app.settings;
                s.notifications = d.notifications;
                s.in_app_toasts = d.in_app_toasts;
                s.working_section = d.working_section;
                s.sidebar_density = d.sidebar_density;
                s.project_order = d.project_order;
                s.delete_confirm = d.delete_confirm;
            }
        });
    });
    let (hr, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(hr.x_range(), hr.top(), Stroke::new(1.0, pal.border));
    egui::ScrollArea::vertical().id_salt(("settings_content", &page, q.is_empty())).auto_shrink([false, false]).show(ui, |ui| {
        let avail = ui.available_width();
        let maxw = (avail - if narrow { 24.0 } else { 48.0 }).min(896.0);
        ui.horizontal(|ui| {
            ui.add_space((avail - maxw) / 2.0);
            ui.vertical(|ui| {
                ui.set_width(maxw);
                ui.add_space(24.0);
                ui.spacing_mut().item_spacing.y = 10.0;
                if q.is_empty() {
                    for s in sections(&page) {
                        render_section(app, ui, &s, s.title, None);
                        ui.add_space(22.0);
                    }
                } else {
                    let mut any = false;
                    for (_, pages) in GROUPS {
                        for pid in *pages {
                            let (pname, _) = page_meta(pid);
                            for s in sections(pid) {
                                let rows: Vec<R> = s
                                    .rows
                                    .into_iter()
                                    .filter(|r| format!("{} {} {pname}", r.text(), s.title).to_lowercase().contains(&q))
                                    .collect();
                                if rows.is_empty() {
                                    continue;
                                }
                                any = true;
                                let t = if s.title.is_empty() { pname.to_string() } else { format!("{pname} · {}", s.title) };
                                let filtered = Sec { title: "", rows, plain: s.plain };
                                render_section(app, ui, &filtered, &t, Some(pid));
                                ui.add_space(22.0);
                            }
                        }
                    }
                    if !any {
                        ui.add_space(32.0);
                        ui.vertical_centered(|ui| {
                            w::text(ui, format!("No settings match “{}”.", app.ui.settings_query.trim()), 14.0, pal.muted)
                        });
                    }
                }
                ui.add_space(48.0);
            });
        });
    });
}

fn render_section(app: &mut App, ui: &mut Ui, s: &Sec, title: &str, jump: Option<&str>) {
    let pal = p();
    if !title.is_empty() {
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            w::text_semibold(ui, title, 16.0, pal.fg);
            if let Some(pid) = jump {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_space(16.0);
                    if ui
                        .add(
                            egui::Label::new(
                                egui::RichText::new("Open page").font(theme::ui_medium(fs(12.0))).color(pal.muted).underline(),
                            )
                            .sense(Sense::click()),
                        )
                        .clicked()
                    {
                        app.ui.settings_page = pid.to_string();
                        app.ui.settings_query.clear();
                    }
                });
            }
        });
    }
    let body = |app: &mut App, ui: &mut Ui| {
        for (i, r) in s.rows.iter().enumerate() {
            if i > 0 && !s.plain {
                w::hairline(ui, pal.border_faint);
            }
            render_row(app, ui, r);
        }
    };
    if s.plain {
        body(app, ui);
    } else {
        Frame::new().fill(pal.card).stroke(Stroke::new(1.0, pal.border_soft)).corner_radius(radius(theme::R_XL)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            body(app, ui);
        });
    }
}

/// Standard row: title (+ badge, + reset), description, control on the right.
fn line(ui: &mut Ui, title: &str, desc: &str, modified: bool, control: impl FnOnce(&mut Ui)) -> bool {
    let pal = p();
    let mut reset = false;
    Frame::new().inner_margin(Margin::symmetric(16, 12)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        let total = ui.available_width();
        let ctrl_w = (total * 0.42).clamp(140.0, 340.0);
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.set_width(total - ctrl_w - 32.0);
                ui.spacing_mut().item_spacing.y = 4.0;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    w::text_medium(ui, title, 14.0, pal.fg);
                    if modified
                        && w::icon_button_sized(ui, ic::UNDO, &format!("Reset {title} to default"), false, 20.0, 12.0, pal.muted).clicked()
                    {
                        reset = true;
                    }
                });
                if !desc.is_empty() {
                    ui.add(egui::Label::new(egui::RichText::new(desc).font(theme::ui(fs(12.0))).color(pal.muted)).wrap());
                }
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.set_width(ctrl_w);
                control(ui);
            });
        });
    });
    reset
}

fn render_row(app: &mut App, ui: &mut Ui, r: &R) {
    let pal = p();
    match r {
        R::Toggle { t, d, get, set, def, badge } => {
            let mut v = get(&app.settings);
            let reset = line(ui, t, d, v != *def, |ui| {
                w::switch(ui, &mut v);
                if let Some(b) = badge {
                    w::tone_badge(ui, b, pal.info);
                }
            });
            set(&mut app.settings, if reset { *def } else { v });
        }
        R::Select { t, d, opts, get, set, def, w: width } => {
            let mut v = get(&app.settings);
            let reset = line(ui, t, d, v != *def, |ui| {
                w::select(ui, *t, &mut v, opts, *width);
            });
            set(&mut app.settings, if reset { def.to_string() } else { v });
        }
        R::Input { t, d, ph, get, set, def, w: width, mono } => {
            let mut v = get(&app.settings);
            let reset = line(ui, t, d, v != *def, |ui| {
                w::text_input(ui, &mut v, ph, *width, *mono);
            });
            set(&mut app.settings, if reset { def.to_string() } else { v });
        }
        R::Range { t, d, get, set, def, min, max, step, unit } => {
            let mut v = get(&app.settings);
            let reset = line(ui, t, d, v != *def, |ui| {
                Frame::new().fill(pal.ctrl_bg).corner_radius(radius(theme::R_MD)).inner_margin(Margin::symmetric(8, 4)).show(ui, |ui| {
                    ui.set_min_width(40.0);
                    w::mono_text(ui, format!("{v}{unit}"), 12.0, pal.fg);
                });
                ui.spacing_mut().slider_width = 200.0;
                ui.add(egui::Slider::new(&mut v, *min..=*max).step_by(*step as f64).show_value(false));
            });
            set(&mut app.settings, if reset { *def } else { v });
        }
        R::Days { t, d, on, set_on, n, set_n, def_on, def_n } => {
            let mut o = on(&app.settings);
            let mut k = n(&app.settings);
            let reset = line(ui, t, d, o != *def_on || k != *def_n, |ui| {
                w::switch(ui, &mut o);
                if o {
                    Frame::new().fill(pal.ctrl_bg).stroke(Stroke::new(1.0, pal.input_border)).corner_radius(radius(theme::R_LG)).show(
                        ui,
                        |ui| {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 2.0;
                                if w::icon_button_sized(ui, ic::PLUS, "More days", false, 26.0, 12.0, pal.muted).clicked() {
                                    k += 1;
                                }
                                w::text(ui, format!("{k} days"), 13.0, pal.fg);
                                if ui
                                    .add(
                                        egui::Button::new(egui::RichText::new("−").color(pal.muted))
                                            .frame(false)
                                            .min_size(vec2(26.0, 26.0)),
                                    )
                                    .on_hover_text("Fewer days")
                                    .clicked()
                                {
                                    k = k.saturating_sub(1).max(1);
                                }
                            });
                        },
                    );
                } else {
                    w::text(ui, "Off", 12.0, pal.muted);
                }
            });
            if reset {
                o = *def_on;
                k = *def_n;
            }
            set_on(&mut app.settings, o);
            set_n(&mut app.settings, k);
        }
        R::Info(text) => {
            Frame::new().inner_margin(Margin::symmetric(16, 12)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                w::row_top(ui, |ui| {
                    w::icon(ui, ic::INFO, 14.0, pal.info);
                    ui.add(egui::Label::new(super::markdown::inline_job(text, fs(12.0), pal.muted)).wrap());
                });
            });
        }
        R::Custom { f, .. } => f(app, ui),
    }
}

// ---------------------------------------------------------------- custom rows

fn scheme_tiles(app: &mut App, ui: &mut Ui) {
    let pal = p();
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        w::text_semibold(ui, "Color scheme", 16.0, pal.fg);
    });
    ui.add_space(10.0);
    let wid = (ui.available_width() - 24.0) / 3.0;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        for (label, glyph) in [("System", ic::MONITOR), ("Light", ic::SUN), ("Dark", ic::MOON)] {
            let on = app.settings.theme == label;
            let dark = label != "Light";
            let r = Frame::new()
                .fill(if on { theme::tint(pal.accent, 0.06) } else { pal.card })
                .stroke(Stroke::new(if on { 2.0 } else { 1.0 }, if on { pal.accent } else { pal.border }))
                .corner_radius(radius(theme::R_XL))
                .inner_margin(8)
                .show(ui, |ui| {
                    ui.set_width(wid - 18.0);
                    ui.vertical(|ui| {
                        let (pv, _) = ui.allocate_exact_size(vec2(wid - 18.0, 120.0), Sense::hover());
                        let (bg, side, bd, line) = if dark {
                            (theme::hex(0), theme::hex(0x111111), theme::rgba(255, 255, 255, 0.1), theme::rgba(255, 255, 255, 0.14))
                        } else {
                            (theme::hex(0xfcfcfc), theme::hex(0xfafafa), theme::hex(0xe4e4e7), theme::hex(0xe4e4e7))
                        };
                        ui.painter().rect(pv, radius(8), bg, Stroke::new(1.0, bd), egui::StrokeKind::Inside);
                        let sr = Rect::from_min_size(pv.min, vec2(pv.width() * 0.3, pv.height()));
                        ui.painter().rect_filled(sr.shrink(1.0), egui::CornerRadius { nw: 7, sw: 7, ne: 0, se: 0 }, side);
                        for (k, fr) in [0.7f32, 0.9, 0.6].iter().enumerate() {
                            ui.painter().rect_filled(
                                Rect::from_min_size(sr.min + vec2(8.0, 10.0 + k as f32 * 12.0), vec2((sr.width() - 16.0) * fr, 6.0)),
                                radius(3),
                                line,
                            );
                        }
                        let mr = Rect::from_min_max(pos2(sr.right() + 10.0, pv.top() + 10.0), pv.max - vec2(10.0, 10.0));
                        ui.painter().rect_filled(
                            Rect::from_min_size(pos2(mr.right() - mr.width() * 0.55, mr.bottom() - 52.0), vec2(mr.width() * 0.55, 14.0)),
                            radius(6),
                            line,
                        );
                        ui.painter().rect_filled(
                            Rect::from_min_size(pos2(mr.left(), mr.bottom() - 32.0), vec2(mr.width() * 0.8, 6.0)),
                            radius(3),
                            line,
                        );
                        ui.painter().rect(
                            Rect::from_min_size(pos2(mr.left(), mr.bottom() - 22.0), vec2(mr.width(), 22.0)),
                            radius(8),
                            side,
                            Stroke::new(1.0, bd),
                            egui::StrokeKind::Inside,
                        );
                        ui.horizontal(|ui| {
                            ui.add_space((wid - 18.0) / 2.0 - 30.0);
                            w::icon(ui, glyph, 12.0, pal.fg);
                            w::text_medium(ui, label, 12.0, pal.fg);
                        });
                    });
                });
            if r.response.interact(Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                app.settings.theme = label.into();
            }
        }
    });
    ui.add_space(22.0);
}

fn theme_tiles(app: &mut App, ui: &mut Ui) {
    let pal = p();
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        w::text_semibold(ui, "Themes", 16.0, pal.fg);
    });
    ui.add_space(10.0);
    let n = 4.0;
    let wid = (ui.available_width() - 8.0 * (n - 1.0)) / n;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        for (name, c2, c3) in
            [("Kiln", 0x0a0a0a, 0xf5f5f5), ("Ember", 0x1c1410, 0xfde7d9), ("Forest", 0x0d1610, 0xdcfce7), ("Violet", 0x130f1c, 0xede9fe)]
        {
            let on = app.settings.theme_name == name;
            let c1 = theme::accent_for_theme(name, true);
            let r = Frame::new()
                .fill(pal.card)
                .stroke(Stroke::new(if on { 2.0 } else { 1.0 }, if on { c1 } else { pal.border_soft }))
                .corner_radius(radius(theme::R_XL))
                .inner_margin(12)
                .show(ui, |ui| {
                    ui.set_width(wid - 26.0);
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            for c in [c1, theme::hex(c2), theme::hex(c3)] {
                                let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
                                ui.painter().circle(r.center(), 9.0, c, Stroke::new(1.0, pal.border));
                            }
                        });
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            w::text_medium(ui, name, 14.0, pal.fg);
                            if on {
                                w::badge(ui, "Active", pal.fg, theme::tint(c1, 0.2));
                            }
                        });
                    });
                });
            if r.response.interact(Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                app.settings.theme_name = name.into();
            }
        }
    });
}

fn keybindings(app: &mut App, ui: &mut Ui) {
    let pal = p();
    Frame::new().inner_margin(Margin::symmetric(16, 12)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            w::text_input(ui, &mut app.ui.kb_query, "Search keybindings", ui.available_width() - 150.0, false);
            if Btn::new("Reset all").icon(ic::ROTATE_CCW).medium().show(ui).clicked() {
                app.settings.keybindings = st::default_keybindings();
            }
        });
    });
    // Capture a new chord.
    if let Some(action) = app.ui.capture_key.clone() {
        let captured = ui.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Key { key, pressed: true, modifiers, .. } if !matches!(key, egui::Key::Escape) || modifiers.any() => {
                    Some(st::chord_label(*modifiers, *key))
                }
                egui::Event::Key { key: egui::Key::Escape, pressed: true, .. } => Some(String::new()),
                _ => None,
            })
        });
        if let Some(c) = captured {
            if !c.is_empty() {
                app.settings.keybindings.insert(action, c);
            }
            app.ui.capture_key = None;
        }
    }
    let q = app.ui.kb_query.to_lowercase();
    let mut group = "";
    for (id, g, title, when) in st::ACTIONS {
        if !q.is_empty() && !title.to_lowercase().contains(&q) {
            continue;
        }
        if *g != group {
            group = g;
            ui.horizontal(|ui| {
                ui.add_space(16.0);
                w::text_medium(ui, *g, 12.0, pal.muted);
            });
        }
        w::hairline(ui, pal.border_faint);
        let chord = app.settings.keybindings.get(*id).cloned().unwrap_or_default();
        let def = st::default_keybindings().get(*id).cloned().unwrap_or_default();
        let capturing = app.ui.capture_key.as_deref() == Some(*id);
        let reset = line(ui, title, &format!("When {when}"), chord != def, |ui| {
            if Btn::new(if capturing { "Press keys…" } else { "Change" }).show(ui).clicked() {
                app.ui.capture_key = Some(id.to_string());
            }
            for k in chord.split('+').rev() {
                w::kbd(ui, k);
            }
        });
        if reset {
            app.settings.keybindings.insert(id.to_string(), def);
        }
    }
}

fn kv(ui: &mut Ui, k: &str, v: &str) {
    let pal = p();
    w::row_top(ui, |ui| {
        let (r, _) = ui.allocate_exact_size(vec2(120.0, 18.0), Sense::hover());
        ui.painter().text(r.left_center(), Align2::LEFT_CENTER, k, theme::ui(fs(12.0)), pal.muted);
        ui.add(egui::Label::new(egui::RichText::new(v).font(theme::ui(fs(13.0))).color(pal.fg)).wrap());
    });
}

fn providers_panel(app: &mut App, ui: &mut Ui) {
    let pal = p();
    let sel = match app.ui.settings_provider {
        crate::app::ProviderKind2::Claude => ProviderKind::Claude,
        crate::app::ProviderKind2::Codex => ProviderKind::Codex,
    };
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        w::text_semibold(ui, "Providers", 16.0, pal.fg);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(16.0);
            let checked = app.providers.values().map(|s| s.checked_at).max().unwrap_or(0);
            if Btn::ghost(&if checked == 0 {
                "Checking…".to_string()
            } else {
                if now() - checked < 60 { "Checked just now".to_string() } else { format!("Checked {} ago", w::ago(checked)) }
            })
            .icon(ic::ROTATE_CCW)
            .show(ui)
            .on_hover_text("Check again (read-only)")
            .clicked()
            {
                app.refresh_providers();
            }
        });
    });
    ui.add_space(10.0);
    Frame::new().fill(pal.card).stroke(Stroke::new(1.0, pal.border_soft)).corner_radius(radius(theme::R_XL)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        w::row_top(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.vertical(|ui| {
                ui.set_width(272.0);
                ui.spacing_mut().item_spacing.y = 0.0;
                for k in ProviderKind::ALL {
                    let s = app.providers.get(&k).cloned().unwrap_or_else(|| provider::ProviderStatus::unknown(k));
                    let on = sel == k;
                    let (rect, resp) = ui.allocate_exact_size(vec2(272.0, 72.0), Sense::click());
                    if on {
                        ui.painter().rect_filled(
                            rect,
                            0.0,
                            if pal.dark { theme::rgba(245, 245, 245, 0.06) } else { theme::rgba(24, 24, 27, 0.04) },
                        );
                    }
                    let tone = if k == ProviderKind::Claude { pal.warning } else { pal.info };
                    w::paint_fav(
                        ui,
                        Rect::from_min_size(pos2(rect.left() + 16.0, rect.center().y - 14.0), vec2(28.0, 28.0)),
                        k.short(),
                        tone,
                        theme::tint(tone, 0.16),
                    );
                    ui.painter().text(
                        pos2(rect.left() + 56.0, rect.center().y - 9.0),
                        Align2::LEFT_CENTER,
                        k.label(),
                        theme::ui_medium(fs(14.0)),
                        pal.fg,
                    );
                    if let Some(v) = &s.version {
                        let lw = w::galley_width(ui, k.label(), &theme::ui_medium(fs(14.0)));
                        ui.painter().text(
                            pos2(rect.left() + 62.0 + lw, rect.center().y - 9.0),
                            Align2::LEFT_CENTER,
                            v,
                            theme::mono(mono_fs(12.0)),
                            pal.muted,
                        );
                    }
                    let dot = if s.ready() {
                        pal.success
                    } else if s.executable.is_none() {
                        pal.muted
                    } else {
                        pal.warning
                    };
                    ui.painter().circle_filled(pos2(rect.left() + 59.0, rect.center().y + 11.0), 3.0, dot);
                    w::paint_truncated(
                        ui,
                        pos2(rect.left() + 68.0, rect.center().y + 11.0),
                        Align2::LEFT_CENTER,
                        &s.summary(),
                        theme::ui(fs(12.0)),
                        pal.muted80,
                        150.0,
                    );
                    let sw = Rect::from_center_size(pos2(rect.right() - 28.0, rect.center().y), vec2(40.0, 28.0));
                    let swr = ui.interact(sw, ui.scope_id().with(("prov_on", k.key())), Sense::click());
                    let enabled = app.provider_enabled(k);
                    w::paint_switch(ui, sw, enabled, false);
                    if swr.clicked() {
                        match k {
                            ProviderKind::Claude => app.settings.claude_enabled = !enabled,
                            ProviderKind::Codex => app.settings.codex_enabled = !enabled,
                        }
                    } else if resp.clicked() {
                        app.ui.settings_provider =
                            if k == ProviderKind::Claude { crate::app::ProviderKind2::Claude } else { crate::app::ProviderKind2::Codex };
                    }
                    if k == ProviderKind::Claude {
                        ui.painter().hline(rect.x_range(), rect.bottom(), Stroke::new(1.0, pal.border_faint));
                    }
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.add_space(16.0);
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(
                                "Kiln drives the CLIs you installed and their own sign-in. Other agents aren't supported yet.",
                            )
                            .font(theme::ui(fs(11.0)))
                            .color(pal.faint),
                        )
                        .wrap(),
                    );
                });
                ui.add_space(8.0);
            });
            let (vr, _) = ui.allocate_exact_size(vec2(1.0, 160.0), Sense::hover());
            ui.painter().vline(vr.center().x, ui.min_rect().y_range(), Stroke::new(1.0, pal.border_faint));
            ui.vertical(|ui| {
                Frame::new().inner_margin(Margin::symmetric(16, 16)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    provider_detail(app, ui, sel);
                });
            });
        });
    });
}

fn provider_detail(app: &mut App, ui: &mut Ui, k: ProviderKind) {
    let pal = p();
    let s = app.providers.get(&k).cloned().unwrap_or_else(|| provider::ProviderStatus::unknown(k));
    ui.spacing_mut().item_spacing.y = 8.0;
    w::text_semibold(ui, k.label(), 16.0, pal.fg);
    ui.horizontal(|ui| {
        let (txt, c) = if s.ready() {
            ("Ready", pal.success)
        } else if s.executable.is_none() {
            ("Not installed", pal.muted)
        } else {
            ("Not ready", pal.warning)
        };
        w::tone_badge(ui, txt, c);
        w::text(ui, s.summary(), 12.0, pal.muted);
    });
    kv(ui, "Executable", &s.executable.as_ref().map(|e| e.display().to_string()).unwrap_or_else(|| "Not found on PATH".into()));
    kv(ui, "Version", s.version.as_deref().unwrap_or("—"));
    match &s.auth {
        AuthState::SignedIn { method, account, plan } => {
            kv(ui, "Signed in with", method);
            if let Some(a) = account {
                kv(ui, "Account", a);
            }
            if let Some(pl) = plan {
                kv(ui, "Plan", pl);
            }
        }
        AuthState::SignedOut => {
            kv(ui, "Sign-in", "Signed out");
        }
        AuthState::Error(e) => kv(ui, "Sign-in", &format!("Couldn't read status: {e}")),
        AuthState::Unknown => kv(ui, "Sign-in", "Unknown"),
    }
    if let Some(e) = &s.error {
        ui.add(egui::Label::new(egui::RichText::new(e).font(theme::ui(fs(12.0))).color(pal.warning)).wrap());
    }
    if s.executable.is_none() {
        ui.add(egui::Label::new(egui::RichText::new(s.install_hint()).font(theme::ui(fs(12.0))).color(pal.muted)).wrap());
    } else if !s.ready() {
        ui.add(
            egui::Label::new(
                egui::RichText::new(format!(
                    "Sign in with the provider's own login: run `{}`. Kiln never asks for your password or an API key.",
                    s.login_command()
                ))
                .font(theme::ui(fs(12.0)))
                .color(pal.muted),
            )
            .wrap(),
        );
        ui.horizontal(|ui| {
            if Btn::new("Open a terminal to sign in").icon(ic::TERMINAL).show(ui).clicked() {
                let cwd = crate::log::home_dir();
                app.open_terminal(cwd, Some(s.login_command().to_string()));
                app.ui.settings_open = false;
                app.toast("Finish signing in in the terminal, then check providers again.", true);
            }
            if Btn::ghost("Copy command").icon(ic::COPY).show(ui).clicked() {
                ui.ctx().copy_text(s.login_command().to_string());
            }
        });
    }
    w::hairline(ui, pal.border_faint);
    let mut path = match k {
        ProviderKind::Claude => app.settings.claude_path.clone(),
        ProviderKind::Codex => app.settings.codex_path.clone(),
    };
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            w::text_medium(ui, "Executable path", 14.0, pal.fg);
            w::text(ui, "Leave empty to find it on PATH.", 12.0, pal.muted);
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            w::text_input(ui, &mut path, "Auto-detect", 220.0, true);
        });
    });
    match k {
        ProviderKind::Claude => app.settings.claude_path = path,
        ProviderKind::Codex => app.settings.codex_path = path,
    }
    let models = app.models_for(k);
    let mut model_ids: Vec<String> = models.iter().map(|m| m.id.clone()).collect();
    if k == ProviderKind::Codex {
        model_ids.insert(0, String::new());
    }
    let (mut model, mut effort) = match k {
        ProviderKind::Claude => (app.settings.claude_model.clone(), app.settings.claude_effort.clone()),
        ProviderKind::Codex => (app.settings.codex_model.clone(), app.settings.codex_effort.clone()),
    };
    let disp = |id: &str| {
        if id.is_empty() {
            "Codex default".to_string()
        } else {
            models.iter().find(|m| m.id == id).map(|m| m.display.clone()).filter(|d| !d.is_empty()).unwrap_or_else(|| id.to_string())
        }
    };
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            w::text_medium(ui, "Default model", 14.0, pal.fg);
            w::text(
                ui,
                if models.is_empty() { "Models appear after the health check." } else { "Used for new threads with this provider." },
                12.0,
                pal.muted,
            );
        });
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let r = Btn::new(&disp(&model)).trailing(ic::CHEVRON_DOWN).medium().show(ui);
            egui::Popup::menu(&r).width(260.0).frame(w::popover(10).inner_margin(4)).show(|ui| {
                ui.set_min_width(250.0);
                for id in &model_ids {
                    if w::menu_item(ui, (*id == model).then_some(ic::CHECK), &disp(id), None, true).clicked() {
                        model = id.clone();
                        ui.close();
                    }
                }
            });
        });
    });
    let efforts: Vec<String> =
        models.iter().find(|m| m.id == model).map(|m| m.efforts.clone()).filter(|e| !e.is_empty()).unwrap_or_else(|| {
            if k == ProviderKind::Claude {
                vec!["low".into(), "medium".into(), "high".into(), "xhigh".into(), "max".into()]
            } else {
                Vec::new()
            }
        });
    if !efforts.is_empty() {
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                w::text_medium(ui, "Reasoning effort", 14.0, pal.fg);
                w::text(ui, "Default effort for new threads. The composer can change it per thread.", 12.0, pal.muted);
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let opts: Vec<&str> = efforts.iter().map(|s| s.as_str()).collect();
                if effort.is_empty() {
                    effort = models.iter().find(|m| m.id == model).and_then(|m| m.default_effort.clone()).unwrap_or_default();
                }
                w::select(ui, ("effort", k.key()), &mut effort, &opts, 140.0);
            });
        });
    }
    match k {
        ProviderKind::Claude => {
            app.settings.claude_model = model;
            app.settings.claude_effort = effort;
        }
        ProviderKind::Codex => {
            app.settings.codex_model = model;
            app.settings.codex_effort = effort;
        }
    }
    w::hairline(ui, pal.border_faint);
    w::text_medium(ui, "What Kiln can do with it", 13.0, pal.fg);
    let c = provider::capabilities(k);
    let caps = [
        (c.resume, "Resume the provider's own session after a restart"),
        (c.interrupt, "Interrupt a running turn"),
        (c.steer, "Steer a running turn with a follow-up"),
        (c.session_rules, "Provider-native “allow for this session” approvals"),
        (c.deny_feedback, "Send feedback with a decline"),
        (c.plan_updates, "Live plan progress"),
        (c.token_usage, "Token and context usage"),
        (c.rate_limits, "Subscription rate-limit status"),
        (c.cost_estimate, "API-equivalent cost estimate (not a subscription charge)"),
        (c.model_list, "Model list from the provider"),
        (c.sandbox, "OS sandbox enforced by the provider"),
    ];
    for (ok, label) in caps {
        ui.horizontal(|ui| {
            w::icon(ui, if ok { ic::CHECK } else { ic::X }, 12.0, if ok { pal.success } else { pal.faint });
            w::text(ui, label, 12.0, if ok { pal.fg } else { pal.faint });
        });
    }
    if !s.rate_limits.is_empty() {
        w::hairline(ui, pal.border_faint);
        w::text_medium(ui, "Rate limits (reported by the provider)", 13.0, pal.fg);
        for r in &s.rate_limits {
            rate_limit_row(ui, r);
        }
    }
}

fn rate_limit_row(ui: &mut Ui, r: &provider::RateLimit) {
    let pal = p();
    ui.horizontal(|ui| {
        w::text(ui, capitalize(&r.label), 12.0, pal.fg);
        if let Some(u) = r.used_percent {
            w::progress(ui, 120.0, u / 100.0, if u > 80.0 { pal.warning } else { pal.accent });
            w::text(ui, format!("{u:.0}% used"), 12.0, pal.muted);
        }
        if let Some(st) = &r.status {
            w::text(ui, st, 12.0, if st == "allowed" { pal.success } else { pal.warning });
        }
        if let Some(t) = r.resets_at {
            let mins = (t - now()).max(0) / 60;
            w::text(ui, format!("resets in {}h {:02}m", mins / 60, mins % 60), 12.0, pal.muted);
        }
    });
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

fn rules_panel(app: &mut App, ui: &mut Ui) {
    let pal = p();
    let rules = app.rules.clone();
    let mut remove = None;
    for (i, r) in rules.iter().enumerate() {
        if i > 0 {
            w::hairline(ui, pal.border_faint);
        }
        let proj = r.project_id.as_deref().and_then(|p| app.project(p)).map(|p| p.name.clone());
        Frame::new().inner_margin(Margin::symmetric(16, 10)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (l, fg, bg) = w::project_colors(proj.as_deref().unwrap_or("All projects"));
                w::fav(ui, if proj.is_some() { &l } else { "*" }, fg, bg, 28.0);
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        w::mono_text(ui, &r.pattern, 13.0, pal.fg);
                        let (t, c) = if r.decision == RuleDecision::Allow { ("Allow", pal.success) } else { ("Block", pal.danger) };
                        w::tone_badge(ui, t, c);
                    });
                    let scope = proj.clone().unwrap_or_else(|| "All projects".into());
                    let detail = if r.builtin {
                        format!("{scope} · built in, can't be removed")
                    } else {
                        format!("{scope} · used {} time{}", r.uses, if r.uses == 1 { "" } else { "s" })
                    };
                    w::text(ui, detail, 12.0, pal.muted);
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if !r.builtin && Btn::new("Remove").show(ui).clicked() {
                        remove = Some(r.id.clone());
                    }
                });
            });
        });
    }
    if let Some(id) = remove {
        app.remove_rule(&id);
    }
    w::hairline(ui, pal.border_faint);
    let key = egui::Id::unique("new_rule");
    let (mut pat, mut dec, mut scope): (String, String, String) =
        ui.data_mut(|d| d.get_temp(key).unwrap_or_else(|| (String::new(), "Allow".into(), "All projects".into())));
    Frame::new().inner_margin(Margin::symmetric(16, 12)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_wrapped(|ui| {
            w::text_input(ui, &mut pat, "Command pattern, e.g. cargo test *", 260.0, true);
            w::select(ui, "rule_decision", &mut dec, &["Allow", "Block"], 100.0);
            let mut scopes: Vec<String> = vec!["All projects".into()];
            scopes.extend(app.projects.iter().map(|p| p.name.clone()));
            let opts: Vec<&str> = scopes.iter().map(|s| s.as_str()).collect();
            w::select(ui, "rule_scope", &mut scope, &opts, 150.0);
            if Btn::primary("Add rule").icon(ic::PLUS).medium().enabled(!pat.trim().is_empty()).show(ui).clicked() {
                let pid = app.projects.iter().find(|p| p.name == scope).map(|p| p.id.clone());
                app.add_rule(pid, pat.trim().to_string(), if dec == "Block" { RuleDecision::Block } else { RuleDecision::Allow });
                pat.clear();
            }
        });
        ui.add(egui::Label::new(egui::RichText::new("`*` matches anything. Allow rules never match commands that chain or redirect (&&, ;, |, >), so a rule can't approve a hidden second command. Kiln applies rules by answering the provider's permission request; the provider enforces it.").font(theme::ui(fs(12.0))).color(pal.muted)).wrap());
    });
    ui.data_mut(|d| d.insert_temp(key, (pat, dec, scope)));
}

fn projects_panel(app: &mut App, ui: &mut Ui) {
    let pal = p();
    let scope = app.ui.settings_scope.clone();
    let mut names: Vec<(String, String)> = vec![("All projects".into(), "All projects".into())];
    names.extend(app.projects.iter().map(|p| (p.id.clone(), p.name.clone())));
    let mut cur = names.iter().find(|(id, _)| *id == scope).map(|(_, n)| n.clone()).unwrap_or_else(|| "All projects".into());
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        w::text(ui, "Applying settings for", 14.0, pal.muted);
        let opts: Vec<&str> = names.iter().map(|(_, n)| n.as_str()).collect();
        if w::select(ui, "project_scope", &mut cur, &opts, 220.0) {
            app.ui.settings_scope =
                names.iter().find(|(_, n)| *n == cur).map(|(id, _)| id.clone()).unwrap_or_else(|| "All projects".into());
        }
    });
    ui.add_space(16.0);
    let Some(proj) = app.projects.iter().find(|p| p.id == app.ui.settings_scope).cloned() else {
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            w::text_semibold(ui, "Projects", 16.0, pal.fg);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(16.0);
                if Btn::ghost("Add project").icon(ic::FOLDER_PLUS).show(ui).clicked() {
                    app.pick_project_folder();
                }
            });
        });
        ui.add_space(10.0);
        w::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            if app.projects.is_empty() {
                Frame::new().inner_margin(16).show(ui, |ui| w::text(ui, "No projects yet.", 13.0, pal.muted));
            }
            for (i, pr) in app.projects.clone().iter().enumerate() {
                if i > 0 {
                    w::hairline(ui, pal.border_faint);
                }
                let n = app.threads.iter().filter(|t| t.project_id.as_deref() == Some(pr.id.as_str())).count();
                Frame::new().inner_margin(Margin::symmetric(16, 12)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        let (l, fg, bg) = w::project_colors(&pr.name);
                        w::fav(ui, &l, fg, bg, 28.0);
                        ui.vertical(|ui| {
                            w::text_medium(ui, &pr.name, 14.0, pal.fg);
                            w::text(
                                ui,
                                format!(
                                    "{} · {n} thread{}{}",
                                    pr.path.display(),
                                    if n == 1 { "" } else { "s" },
                                    if pr.archived { " · archived" } else { "" }
                                ),
                                12.0,
                                pal.muted,
                            );
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if Btn::new("Manage").show(ui).clicked() {
                                app.ui.settings_scope = pr.id.clone();
                            }
                        });
                    });
                });
            }
        });
        return;
    };
    // One project
    let mut name = proj.name.clone();
    let mut pinned = proj.pinned;
    w::card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        line(ui, "Name", "Shown in the sidebar, breadcrumbs and pickers.", false, |ui| {
            w::text_input(ui, &mut name, "Name", 224.0, false);
        });
        w::hairline(ui, pal.border_faint);
        line(ui, "Location", &proj.path.display().to_string(), false, |ui| {
            if Btn::new("Open folder").icon(ic::FOLDER).show(ui).clicked() {
                let _ = open::that(&proj.path);
            }
        });
        w::hairline(ui, pal.border_faint);
        line(ui, "Pin", "Pinned projects come first in project pickers.", false, |ui| {
            w::switch(ui, &mut pinned);
        });
    });
    ui.add_space(22.0);
    ui.horizontal(|ui| {
        ui.add_space(16.0);
        w::text_semibold(ui, "Actions", 16.0, pal.fg);
    });
    ui.add_space(10.0);
    let mut scripts = proj.scripts.clone();
    let mut remove = None;
    w::card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        Frame::new().inner_margin(Margin::symmetric(16, 10)).show(ui, |ui| {
            ui.add(egui::Label::new(egui::RichText::new("Commands shown in Thread details. They run in a Kiln terminal in the thread's worktree or checkout. A script named “test” is used for local test runs.").font(theme::ui(fs(12.0))).color(pal.muted)).wrap());
        });
        for (i, (n, c)) in scripts.iter().enumerate() {
            w::hairline(ui, pal.border_faint);
            line(ui, n, c, false, |ui| {
                if Btn::new("Remove").show(ui).clicked() {
                    remove = Some(i);
                }
            });
        }
        w::hairline(ui, pal.border_faint);
        let key = egui::Id::unique(("new_script", &proj.id));
        let (mut sn, mut sc): (String, String) = ui.data_mut(|d| d.get_temp(key).unwrap_or_default());
        Frame::new().inner_margin(Margin::symmetric(16, 12)).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                w::text_input(ui, &mut sn, "Name, e.g. test", 140.0, false);
                w::text_input(ui, &mut sc, "Command, e.g. cargo test", 280.0, true);
                if Btn::primary("Add action").icon(ic::PLUS).medium().enabled(!sn.trim().is_empty() && !sc.trim().is_empty()).show(ui).clicked() {
                    scripts.push((sn.trim().to_string(), sc.trim().to_string()));
                    sn.clear();
                    sc.clear();
                }
            });
        });
        ui.data_mut(|d| d.insert_temp(key, (sn, sc)));
    });
    if let Some(i) = remove {
        scripts.remove(i);
    }
    ui.add_space(22.0);
    let mut archive = None;
    let mut delete = false;
    w::card().show(ui, |ui| {
        ui.set_width(ui.available_width());
        line(
            ui,
            if proj.archived { "Unarchive" } else { "Archive" },
            "Hides the project from pickers. Threads and files are kept.",
            false,
            |ui| {
                if Btn::new(if proj.archived { "Unarchive" } else { "Archive" }).show(ui).clicked() {
                    archive = Some(!proj.archived);
                }
            },
        );
        w::hairline(ui, pal.border_faint);
        line(
            ui,
            "Remove project",
            "Removes the project from Kiln. Its threads stay in the archive list without a project. Files on disk are not touched.",
            false,
            |ui| {
                if Btn::new("Remove project").kind(w::Kind::Danger).show(ui).clicked() {
                    delete = true;
                }
            },
        );
    });
    if let Some(pr) = app.projects.iter_mut().find(|p| p.id == proj.id) {
        let changed = pr.name != name || pr.pinned != pinned || pr.scripts != scripts || archive.is_some();
        if !name.trim().is_empty() {
            pr.name = name;
        }
        pr.pinned = pinned;
        pr.scripts = scripts;
        if let Some(a) = archive {
            pr.archived = a;
        }
        if changed {
            let pr = pr.clone();
            app.store.save_project(&pr);
        }
    }
    if delete {
        app.store.delete_project(&proj.id);
        app.projects.retain(|p| p.id != proj.id);
        for t in app.threads.iter_mut().filter(|t| t.project_id.as_deref() == Some(proj.id.as_str())) {
            t.project_id = None;
        }
        app.ui.settings_scope = "All projects".into();
        app.draft.project = app.projects.first().map(|p| p.id.clone());
    }
}

fn vcs_panel(app: &mut App, ui: &mut Ui) {
    let pal = p();
    line(ui, "Git", app.git_version.as_deref().map(|v| format!("git {v}")).as_deref().unwrap_or("Checking…"), false, |ui| {
        if app.git_version.is_some() {
            w::tone_badge(ui, "Installed", pal.success);
        } else {
            w::tone_badge(ui, "Not found", pal.warning);
        }
    });
    w::hairline(ui, pal.border_faint);
    let (desc, badge) = match app.gh.as_ref() {
        Some((crate::git::GhAuth::SignedIn { user, host }, v)) => {
            (format!("gh {} · {user} on {host}", v.clone().unwrap_or_default()), Some(("Authenticated", pal.success)))
        }
        Some((crate::git::GhAuth::SignedOut(m), v)) => (
            format!(
                "gh {} · signed out. Run `gh auth login` in a terminal. {}",
                v.clone().unwrap_or_default(),
                crate::provider::short(m, 80)
            ),
            Some(("Signed out", pal.warning)),
        ),
        Some((crate::git::GhAuth::NotInstalled, _)) => {
            ("GitHub CLI (gh) is not installed. Pull requests and GitHub suggestions need it.".into(), Some(("Not installed", pal.muted)))
        }
        None => ("Checking…".into(), None),
    };
    line(ui, "GitHub", &desc, false, |ui| {
        if Btn::ghost("Rescan").icon(ic::ROTATE_CCW).show(ui).clicked() {
            app.refresh_gh();
        }
        if let Some((b, c)) = badge {
            w::tone_badge(ui, b, c);
        }
    });
}

fn branch_preview(app: &mut App, ui: &mut Ui) {
    let pal = p();
    let prefix = app.settings.branch_prefix.trim().trim_end_matches('/').to_string();
    let slug = crate::git::slugify("Add search to the thread list", 40);
    let name = if prefix.is_empty() { slug } else { format!("{prefix}/{slug}") };
    line(ui, "Preview", "For the first message “Add search to the thread list”.", false, |ui| {
        Frame::new()
            .fill(theme::rgba(255, 255, 255, 0.03))
            .stroke(Stroke::new(1.0, pal.border))
            .corner_radius(radius(theme::R_MD))
            .inner_margin(Margin::symmetric(8, 4))
            .show(ui, |ui| {
                w::mono_text(ui, &name, 12.0, pal.fg);
            });
    });
}

fn cleanup_panel(app: &mut App, ui: &mut Ui) {
    let pal = p();
    let root = app.worktree_root();
    line(
        ui,
        "Clean up now",
        &format!(
            "See exactly what goes before anything is deleted. Only worktrees under {} are considered; ones with local changes are never removed.",
            root.display()
        ),
        false,
        |ui| {
            if Btn::new(if app.ui.cleanup_preview.is_some() { "Rescan" } else { "Preview cleanup" }).show(ui).clicked() {
                app.scan_worktrees();
            }
        },
    );
    if let Some(list) = app.ui.cleanup_preview.clone() {
        if list.is_empty() {
            w::hairline(ui, pal.border_faint);
            Frame::new().inner_margin(Margin::symmetric(16, 12)).show(ui, |ui| w::text(ui, "No Kiln worktrees found.", 13.0, pal.muted));
        }
        let in_use: std::collections::HashSet<String> = app
            .threads
            .iter()
            .filter(|t| !t.archived && !t.settled)
            .filter_map(|t| t.cwd.clone())
            .map(|c| c.to_string_lossy().to_lowercase())
            .collect();
        let mut removable = Vec::new();
        for (wt, dirty, project) in &list {
            w::hairline(ui, pal.border_faint);
            let active = in_use.contains(&wt.path.to_string_lossy().to_lowercase());
            let status = if *dirty {
                "has local changes · kept"
            } else if active {
                "used by an open thread · kept"
            } else {
                "clean · can be removed"
            };
            if !*dirty && !active {
                removable.push((wt.path.clone(), project.clone()));
            }
            line(
                ui,
                &wt.branch.clone().unwrap_or_else(|| "(detached)".into()),
                &format!("{project} · {} · {status}", wt.path.display()),
                false,
                |ui| {
                    if *dirty {
                        w::tone_badge(ui, "Dirty", pal.warning);
                    } else if active {
                        w::tone_badge(ui, "In use", pal.info);
                    }
                },
            );
        }
        if !removable.is_empty() {
            w::hairline(ui, pal.border_faint);
            let n = removable.len();
            line(
                ui,
                &format!("Remove {n} worktree{}", if n == 1 { "" } else { "s" }),
                "Branches stay. Thread history is kept.",
                false,
                |ui| {
                    if Btn::new("Remove worktrees…").kind(w::Kind::Danger).show(ui).clicked() {
                        app.confirm = Some(Confirm::CleanupWorktrees { items: removable.clone() });
                    }
                },
            );
        }
    }
}

fn usage_panel(app: &mut App, ui: &mut Ui) {
    let pal = p();
    let day = now() - 86_400;
    let month = now() - 30 * 86_400;
    let today = app.threads.iter().filter(|t| t.updated_at >= day).count();
    let monthly = app.threads.iter().filter(|t| t.updated_at >= month).count();
    let tokens: u64 = app.threads.iter().map(|t| t.usage.total_input + t.usage.total_output).sum();
    let cached: u64 = app.threads.iter().map(|t| t.usage.cached_input).sum();
    let est: f64 = app.threads.iter().filter_map(|t| t.usage.est_cost_usd).sum();
    let tile = |ui: &mut Ui, label: &str, value: String, sub: String| {
        Frame::new().inner_margin(Margin::symmetric(16, 14)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 2.0;
            w::text(ui, label, 12.0, pal.muted);
            w::text_semibold(ui, value, 22.0, pal.fg);
            w::text(ui, sub, 12.0, pal.muted);
        });
    };
    ui.columns(3, |c| {
        tile(&mut c[0], "Active threads", format!("{today}"), format!("today · {monthly} in 30 days"));
        tile(&mut c[1], "Tokens", fmt_tokens(tokens), format!("provider-reported · {} cached", fmt_tokens(cached)));
        tile(&mut c[2], "Claude API-equivalent", format!("${est:.2}"), "estimate, not a subscription charge".into());
    });
    for k in ProviderKind::ALL {
        w::hairline(ui, pal.border_faint);
        let n = app.threads.iter().filter(|t| t.provider == k).count();
        let tk: u64 = app.threads.iter().filter(|t| t.provider == k).map(|t| t.usage.total_input + t.usage.total_output).sum();
        let st = app.providers.get(&k).cloned();
        let plan = match st.as_ref().map(|s| &s.auth) {
            Some(AuthState::SignedIn { plan: Some(pl), method, .. }) => format!(" · {method} {pl} plan"),
            Some(AuthState::SignedIn { method, .. }) => format!(" · {method}"),
            _ => String::new(),
        };
        line(ui, k.label(), &format!("{n} thread{} · {} tokens{plan}", if n == 1 { "" } else { "s" }, fmt_tokens(tk)), false, |_| {});
        if let Some(s) = st {
            for r in &s.rate_limits {
                Frame::new().inner_margin(Margin { left: 16, right: 16, top: 0, bottom: 8 }).show(ui, |ui| rate_limit_row(ui, r));
            }
            if s.rate_limits.is_empty() {
                Frame::new()
                    .inner_margin(Margin { left: 16, right: 16, top: 0, bottom: 10 })
                    .show(ui, |ui| w::text(ui, "No rate-limit data reported yet. It appears after a turn.", 12.0, pal.faint));
            }
        }
    }
    w::hairline(ui, pal.border_faint);
    Frame::new().inner_margin(Margin::symmetric(16, 12)).show(ui, |ui| {
        ui.add(egui::Label::new(egui::RichText::new("Token counts and limits come from the providers. Claude Code reports an API-equivalent cost estimate for each turn; with a Claude subscription you are not charged that amount. Kiln doesn't calculate prices itself.").font(theme::ui(fs(12.0))).color(pal.muted)).wrap());
    });
}

fn fmt_tokens(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1} M", n as f64 / 1e6)
    } else if n >= 1000 {
        format!("{:.1} k", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

fn maintenance_panel(app: &mut App, ui: &mut Ui) {
    let pal = p();
    w::hairline(ui, pal.border_faint);
    line(ui, "Logs folder", &crate::log::logs_dir().display().to_string(), false, |ui| {
        if Btn::new("Delete old logs now").show(ui).clicked() {
            let freed = crate::log::prune(app.settings.log_retention_days.max(1));
            app.toast(format!("Freed {} KB of old logs.", freed / 1024), true);
        }
        if Btn::new("Open").icon(ic::FOLDER).show(ui).clicked() {
            let _ = open::that(crate::log::logs_dir());
        }
    });
    w::hairline(ui, pal.border_faint);
    line(ui, "Data folder", &format!("{} · database {}", crate::log::data_dir().display(), app.store.path.display()), false, |ui| {
        if Btn::new("Open").icon(ic::FOLDER).show(ui).clicked() {
            let _ = open::that(crate::log::data_dir());
        }
    });
    w::hairline(ui, pal.border_faint);
    line(ui, "Provider sessions", &format!("{} running · {} terminals open", app.sessions.len(), app.terminals.len()), false, |_| {});
}

fn about_panel(app: &mut App, ui: &mut Ui) {
    let pal = p();
    line(
        ui,
        "Version",
        &format!(
            "Kiln Code {} · {} build · native egui/wgpu",
            env!("CARGO_PKG_VERSION"),
            if cfg!(debug_assertions) { "debug" } else { "release" }
        ),
        false,
        |_| {},
    );
    w::hairline(ui, pal.border_faint);
    line(ui, "Account", "None. Kiln has no login, registration, license or cloud service. Providers use their own sign-in.", false, |_| {});
    w::hairline(ui, pal.border_faint);
    line(
        ui,
        "Open source",
        "Built with egui, eframe, wgpu, rusqlite (SQLite), portable-pty, vt100, syntect, pulldown-cmark, rfd and notify-rust. Their licenses ship with the source (Cargo.lock lists every crate).",
        false,
        |_| {},
    );
    w::hairline(ui, pal.border_faint);
    line(ui, "Data", &crate::log::data_dir().display().to_string(), false, |ui| {
        if Btn::new("Open").icon(ic::FOLDER).show(ui).clicked() {
            let _ = open::that(crate::log::data_dir());
        }
    });
    let _ = app;
}

fn archive_panel(app: &mut App, ui: &mut Ui) {
    let pal = p();
    let archived: Vec<crate::model::Thread> = app.threads.iter().filter(|t| t.archived).cloned().collect();
    if archived.is_empty() {
        ui.add_space(32.0);
        ui.vertical_centered(|ui| w::text(ui, "No archived threads. Archived threads will appear here.", 14.0, pal.muted));
        return;
    }
    let mut groups: std::collections::BTreeMap<String, Vec<crate::model::Thread>> = Default::default();
    for t in archived {
        groups.entry(app.project_name(&t)).or_default().push(t);
    }
    for (proj, list) in groups {
        ui.horizontal(|ui| {
            ui.add_space(16.0);
            let (l, fg, bg) = w::project_colors(&proj);
            w::fav(ui, &l, fg, bg, 16.0);
            w::text_semibold(ui, &proj, 16.0, pal.fg);
        });
        ui.add_space(10.0);
        w::card().show(ui, |ui| {
            ui.set_width(ui.available_width());
            for (i, t) in list.iter().enumerate() {
                if i > 0 {
                    w::hairline(ui, pal.border_faint);
                }
                line(ui, &t.title, &format!("Updated {} ago · created {} ago", w::ago(t.updated_at), w::ago(t.created_at)), false, |ui| {
                    if Btn::new("Delete…").kind(w::Kind::Danger).show(ui).clicked() {
                        app.confirm = Some(Confirm::DeleteThread { thread: t.id.clone() });
                    }
                    if Btn::new("Unarchive").icon(ic::ARCHIVE_X).show(ui).clicked() {
                        app.set_archived(&t.id, false);
                    }
                });
            }
        });
        ui.add_space(22.0);
    }
}
