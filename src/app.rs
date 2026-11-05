//! Application state, background work and event routing. Views live in `ui/`.

use crate::git;
use crate::model::*;
use crate::provider::{self, ApprovalKind, ApprovalRequest, Cmd, Decision, Ev, ProviderEvent, ProviderStatus, SessionHandle, TurnOutcome};
use crate::settings::Settings;
use crate::store::Store;
use crate::terminal::TerminalSession;
use crate::theme;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};

#[derive(Clone, Debug, PartialEq)]
pub enum View {
    Thread,
    NewThread,
    Compare(String),
    Review,
    Board,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RightTab {
    Diff,
    Timeline,
}

/// Diff with background-computed syntax spans, one `Vec<Spans>` per file (flattened hunk lines).
pub struct DiffDoc {
    pub files: Vec<git::FileDiff>,
    pub spans: Vec<Vec<crate::highlight::Spans>>,
    pub fingerprints: Vec<String>,
}

#[derive(Default)]
pub struct GitState {
    pub loading: bool,
    pub changes: Vec<git::FileChange>,
    pub diff: Option<Arc<DiffDoc>>,
    pub diff_loading: bool,
    pub error: Option<String>,
    pub refresh_at: Option<f64>,
    pub loaded_at: f64,
    /// Diff against a checkpoint instead of the thread's base ("Diff since here").
    pub diff_base: Option<(String, String)>,
}

impl GitState {
    pub fn totals(&self) -> (u32, u32, u32) {
        let (a, d) = self.changes.iter().fold((0, 0), |(a, d), c| (a + c.additions, d + c.deletions));
        (self.changes.len() as u32, a, d)
    }
}

pub struct Toast {
    pub text: String,
    pub ok: bool,
    pub until: f64,
    pub link_rules: bool,
}

#[derive(Clone, Debug)]
pub enum Confirm {
    RestoreCheckpoint { thread: String, cp: Checkpoint, changes: Vec<git::FileChange> },
    RemoveWorktree { thread: String },
    DeleteThread { thread: String },
    RevertHunk { thread: String, file: usize, hunk: usize },
    Merge { thread: String, url: String },
    CleanupWorktrees { items: Vec<(PathBuf, String)> },
}

pub struct NewThreadDraft {
    pub project: Option<String>,
    pub prompt: String,
    pub attachments: Vec<PathBuf>,
    pub provider: ProviderKind,
    pub model: String,
    pub effort: String,
    pub worktree: bool,
    pub base_branch: String,
    pub runs: Vec<(ProviderKind, String)>,
    pub compare: bool,
    pub no_project: bool,
}

#[derive(Clone, Debug)]
pub struct Starter {
    pub tag: String,
    pub tone: egui::Color32,
    pub title: String,
    pub meta: String,
    pub prompt: String,
}

#[derive(Default)]
pub struct UiState {
    pub sidebar_open: bool,
    pub details_open: bool,
    pub right_open: bool,
    pub right_tab: Option<RightTab>,
    pub terminal_open: bool,
    pub settings_open: bool,
    pub settings_page: String,
    pub settings_query: String,
    pub settings_scope: String,
    pub palette_open: bool,
    pub palette_query: String,
    pub sidebar_query: String,
    pub sidebar_project: Option<String>,
    pub collapsed: HashSet<String>,
    pub drafts: HashMap<String, String>,
    pub attachments: HashMap<String, Vec<PathBuf>>,
    pub folds: HashSet<String>,
    pub board_filter: Option<String>,
    pub review_file: usize,
    pub comment_line: Option<(usize, u32, bool)>,
    pub comment_draft: String,
    pub selected_checkpoint: Option<usize>,
    pub diff_split: Option<bool>,
    pub diff_wrap: Option<bool>,
    pub collapsed_files: HashSet<String>,
    pub term_active: usize,
    pub capture_key: Option<String>,
    pub kb_query: String,
    pub rename: Option<(String, String)>,
    pub settings_provider: ProviderKind2,
    pub cleanup_preview: Option<Vec<(git::WorktreeInfo, bool, String)>>,
    pub composer_h: f32,
    pub scroll_to_bottom: bool,
    pub focus_composer: bool,
    pub pr_title: HashMap<String, String>,
    pub pr_body: HashMap<String, String>,
    pub pr_base: HashMap<String, String>,
    pub pr_draft: HashMap<String, bool>,
    pub busy: HashSet<String>,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind2 {
    #[default]
    Claude,
    Codex,
}

pub enum Bg {
    Health(ProviderStatus),
    Gh(git::GhAuth, Option<String>, Option<String>),
    Status { thread: String, result: Result<Vec<git::FileChange>, String> },
    Diff { thread: String, result: Result<DiffDoc, String> },
    SessionReady { thread: String, result: Result<SessionHandle, String> },
    WorkspaceReady { thread: String, result: Result<(PathBuf, Option<String>, Option<String>), String> },
    Branches { project: String, list: Vec<String>, default: String },
    Starters { project: String, list: Vec<Starter> },
    Checkpoint { thread: String, result: Result<Checkpoint, String> },
    RestorePreview { thread: String, cp: Checkpoint, result: Result<Vec<git::FileChange>, String> },
    Done { thread: Option<String>, ok: bool, text: String, kind: DoneKind },
    Picked { purpose: Pick, paths: Vec<PathBuf> },
    Tests { thread: String, ok: Option<bool>, summary: String },
    Worktrees(Vec<(git::WorktreeInfo, bool, String)>),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DoneKind {
    Commit,
    Push,
    Pr,
    Restore,
    Merge,
    Revert,
    Remove,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Pick {
    Project,
    Attach(Option<usize>),
}

enum Outgoing {
    Message { text: String, attachments: Vec<PathBuf> },
}

pub struct App {
    pub ctx: egui::Context,
    pub store: Store,
    pub settings: Settings,
    saved_settings: Settings,
    pub projects: Vec<Project>,
    pub threads: Vec<Thread>,
    pub items: HashMap<String, Vec<Item>>,
    live: HashMap<String, HashMap<String, usize>>,
    pub sessions: HashMap<String, SessionHandle>,
    starting: HashSet<String>,
    outbox: HashMap<String, Vec<Outgoing>>,
    pub approvals: HashMap<String, Vec<ApprovalRequest>>,
    pub rules: Vec<ApprovalRule>,
    pub providers: HashMap<ProviderKind, ProviderStatus>,
    pub gh: Option<(git::GhAuth, Option<String>)>,
    pub git_version: Option<String>,
    ev_tx: mpsc::Sender<ProviderEvent>,
    ev_rx: mpsc::Receiver<ProviderEvent>,
    bg_tx: mpsc::Sender<Bg>,
    bg_rx: mpsc::Receiver<Bg>,
    pub view: View,
    pub current: Option<String>,
    pub ui: UiState,
    pub git: HashMap<String, GitState>,
    pub terminals: Vec<(String, TerminalSession)>,
    pub toasts: Vec<Toast>,
    pub draft: NewThreadDraft,
    pub checkpoints: HashMap<String, Vec<Checkpoint>>,
    pub comments: HashMap<String, Vec<ReviewComment>>,
    pub viewed: HashMap<String, HashMap<String, String>>,
    pub confirm: Option<Confirm>,
    pub starters: HashMap<String, Option<Vec<Starter>>>,
    pub branches: HashMap<String, (Vec<String>, String)>,
    health_next: f64,
    pub demo: bool,
    pub focused: bool,
    seq: i64,
    pub system_dark: bool,
}

impl App {
    pub fn new(ctx: egui::Context, store: Store, demo: bool) -> Self {
        let settings = store.load_settings();
        let (ev_tx, ev_rx) = mpsc::channel();
        let (bg_tx, bg_rx) = mpsc::channel();
        let projects = store.load_projects();
        let mut threads = store.load_threads();
        let mut items: HashMap<String, Vec<Item>> = HashMap::new();
        // Crash/restart recovery: nothing is running now. Never mark interrupted work as done.
        for t in &mut threads {
            if t.state.is_active() {
                t.state = t.state.after_restart();
                t.turn_started_at = None;
                t.last_activity = Some("Interrupted when Kiln closed".into());
                let mut list = store.load_items(&t.id);
                let seq = list.last().map(|i| i.seq + 1).unwrap_or(0);
                let it = Item { id: new_id(), thread_id: t.id.clone(), seq, created_at: now(), kind: ItemKind::Notice { text: "Kiln closed while this run was active, so the run stopped. Nothing was retried automatically. Send a message to continue; Kiln resumes the provider's own session.".into() } };
                store.save_item(&it);
                list.push(it);
                items.insert(t.id.clone(), list);
                store.save_thread(t);
            }
        }
        let mut rules = store.load_rules();
        if !rules.iter().any(|r| r.builtin) {
            let r = ApprovalRule {
                id: "builtin-rm-rf".into(),
                project_id: None,
                pattern: "rm -rf *".into(),
                decision: RuleDecision::Block,
                created_at: now(),
                uses: 0,
                builtin: true,
            };
            store.save_rule(&r);
            rules.push(r);
        }
        let ui_json: serde_json::Value = store.get_kv("ui").and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        let current = ui_json["thread"].as_str().map(str::to_string).filter(|id| threads.iter().any(|t| &t.id == id));
        let mut ui = UiState {
            sidebar_open: true,
            details_open: ui_json["details"].as_bool().unwrap_or(true),
            right_open: ui_json["right"].as_bool().unwrap_or(false),
            terminal_open: false,
            settings_page: "general".into(),
            settings_scope: "All projects".into(),
            ..Default::default()
        };
        ui.collapsed.insert("settled".into());
        let draft = NewThreadDraft {
            project: ui_json["project"]
                .as_str()
                .map(str::to_string)
                .filter(|id| projects.iter().any(|p| &p.id == id))
                .or_else(|| projects.first().map(|p| p.id.clone())),
            prompt: String::new(),
            attachments: Vec::new(),
            provider: ProviderKind::from_key(&settings.default_provider).unwrap_or(ProviderKind::Claude),
            model: String::new(),
            effort: String::new(),
            worktree: settings.workspace == "New worktree",
            base_branch: String::new(),
            runs: Vec::new(),
            compare: false,
            no_project: false,
        };
        let mut providers = HashMap::new();
        for k in ProviderKind::ALL {
            providers.insert(k, ProviderStatus::unknown(k));
        }
        let view = if current.is_some() { View::Thread } else { View::NewThread };
        let mut app = Self {
            ctx,
            store,
            saved_settings: settings.clone(),
            settings,
            projects,
            threads,
            items,
            live: HashMap::new(),
            sessions: HashMap::new(),
            starting: HashSet::new(),
            outbox: HashMap::new(),
            approvals: HashMap::new(),
            rules,
            providers,
            gh: None,
            git_version: None,
            ev_tx,
            ev_rx,
            bg_tx,
            bg_rx,
            view,
            current,
            ui,
            git: HashMap::new(),
            terminals: Vec::new(),
            toasts: Vec::new(),
            draft,
            checkpoints: HashMap::new(),
            comments: HashMap::new(),
            viewed: HashMap::new(),
            confirm: None,
            starters: HashMap::new(),
            branches: HashMap::new(),
            health_next: 0.0,
            demo,
            focused: true,
            seq: 0,
            system_dark: true,
        };
        if let Some(id) = app.current.clone() {
            app.open_thread(&id);
        }
        app.refresh_providers();
        app.refresh_gh();
        let pids: Vec<String> = app.projects.iter().map(|p| p.id.clone()).collect();
        for p in pids {
            app.load_branches(&p);
        }
        crate::log::prune(app.settings.log_retention_days);
        app
    }

    // ------------------------------------------------------------------ helpers

    fn bg(&self, f: impl FnOnce() -> Bg + Send + 'static) {
        let tx = self.bg_tx.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(f());
            ctx.request_repaint();
        });
    }

    pub fn time(&self) -> f64 {
        self.ctx.input(|i| i.time)
    }

    pub fn toast(&mut self, text: impl Into<String>, ok: bool) {
        let t = self.time();
        self.toasts.push(Toast { text: text.into(), ok, until: t + 4.5, link_rules: false });
    }

    pub fn thread(&self, id: &str) -> Option<&Thread> {
        self.threads.iter().find(|t| t.id == id)
    }
    pub fn thread_mut(&mut self, id: &str) -> Option<&mut Thread> {
        self.threads.iter_mut().find(|t| t.id == id)
    }
    pub fn project(&self, id: &str) -> Option<&Project> {
        self.projects.iter().find(|p| p.id == id)
    }
    pub fn project_name(&self, t: &Thread) -> String {
        t.project_id.as_deref().and_then(|p| self.project(p)).map(|p| p.name.clone()).unwrap_or_else(|| "No project".into())
    }

    fn save_thread(&self, id: &str) {
        if let Some(t) = self.thread(id) {
            self.store.save_thread(t);
        }
    }

    fn push_item(&mut self, thread: &str, kind: ItemKind) -> usize {
        self.seq += 1;
        let list = self.items.entry(thread.to_string()).or_insert_with(|| self.store.load_items(thread));
        let seq = list.last().map(|i| i.seq + 1).unwrap_or(0).max(self.seq);
        let it = Item { id: new_id(), thread_id: thread.to_string(), seq, created_at: now(), kind };
        self.store.save_item(&it);
        list.push(it);
        list.len() - 1
    }

    fn persist_item(&self, thread: &str, idx: usize) {
        if let Some(it) = self.items.get(thread).and_then(|l| l.get(idx)) {
            self.store.save_item(it);
        }
    }

    pub fn save_ui(&self) {
        let v = serde_json::json!({
            "thread": self.current,
            "project": self.draft.project,
            "details": self.ui.details_open,
            "right": self.ui.right_open,
        });
        self.store.set_kv("ui", v.to_string());
    }

    pub fn cwd_of(&self, t: &Thread) -> Option<PathBuf> {
        t.cwd.clone().or_else(|| t.project_id.as_deref().and_then(|p| self.project(p)).map(|p| p.path.clone()))
    }

    pub fn worktree_root(&self) -> PathBuf {
        let s = self.settings.worktree_location.trim();
        if s.is_empty() {
            crate::log::default_worktree_root()
        } else if let Some(rest) = s.strip_prefix("~/").or_else(|| s.strip_prefix("~\\")) {
            crate::log::home_dir().join(rest)
        } else {
            PathBuf::from(s)
        }
    }

    // ------------------------------------------------------------------ providers

    pub fn refresh_providers(&mut self) {
        for k in ProviderKind::ALL {
            let path = match k {
                ProviderKind::Claude => self.settings.claude_path.clone(),
                ProviderKind::Codex => self.settings.codex_path.clone(),
            };
            self.bg(move || Bg::Health(provider::check(k, &path)));
        }
        self.health_next = self.time() + self.settings.health_interval_secs.max(30) as f64;
    }

    pub fn refresh_gh(&self) {
        self.bg(|| Bg::Gh(git::gh_auth_status(), git::gh_version(), git::git_version().ok()));
    }

    pub fn provider_enabled(&self, k: ProviderKind) -> bool {
        match k {
            ProviderKind::Claude => self.settings.claude_enabled,
            ProviderKind::Codex => self.settings.codex_enabled,
        }
    }

    pub fn models_for(&self, k: ProviderKind) -> Vec<provider::ModelInfo> {
        let m = self.providers.get(&k).map(|s| s.models.clone()).unwrap_or_default();
        if m.is_empty() && k == ProviderKind::Claude { provider::claude::model_aliases() } else { m }
    }

    pub fn default_model(&self, k: ProviderKind) -> (String, String) {
        match k {
            ProviderKind::Claude => (self.settings.claude_model.clone(), self.settings.claude_effort.clone()),
            ProviderKind::Codex => {
                let models = self.models_for(k);
                let m = if self.settings.codex_model.is_empty() {
                    models.iter().find(|m| m.is_default).map(|m| m.id.clone()).unwrap_or_default()
                } else {
                    self.settings.codex_model.clone()
                };
                let e = if self.settings.codex_effort.is_empty() {
                    models.iter().find(|x| x.id == m).and_then(|x| x.default_effort.clone()).unwrap_or_default()
                } else {
                    self.settings.codex_effort.clone()
                };
                (m, e)
            }
        }
    }

    pub fn model_label(&self, k: ProviderKind, model: &str) -> String {
        let models = self.models_for(k);
        match models.iter().find(|m| m.id == model) {
            Some(m) if !m.display.is_empty() => {
                if k == ProviderKind::Claude && m.id == "default" {
                    "Claude Code default".into()
                } else {
                    m.display.replace(" (latest)", "")
                }
            }
            _ if model.is_empty() => format!("{} default", k.label()),
            _ => model.to_string(),
        }
    }

    // ------------------------------------------------------------------ projects

    pub fn pick_project_folder(&self) {
        let start = if self.settings.add_project_start.trim().is_empty() {
            crate::log::home_dir()
        } else {
            PathBuf::from(self.settings.add_project_start.trim().replace('~', &crate::log::home_dir().to_string_lossy()))
        };
        self.bg(move || {
            let p = rfd::FileDialog::new().set_title("Add a project").set_directory(start).pick_folder();
            Bg::Picked { purpose: Pick::Project, paths: p.into_iter().collect() }
        });
    }

    pub fn pick_attachments(&self, target: Option<usize>) {
        self.bg(move || {
            let p = rfd::FileDialog::new().set_title("Attach files").pick_files().unwrap_or_default();
            Bg::Picked { purpose: Pick::Attach(target), paths: p }
        });
    }

    pub fn add_project(&mut self, path: &Path) {
        let root = match git::repo_root(path) {
            Ok(r) => r,
            Err(e) => {
                self.toast(format!("{} is not a Git repository: {}", path.display(), e.message), false);
                return;
            }
        };
        if let Some(p) = self.projects.iter().find(|p| same_path(&p.path, &root)) {
            let id = p.id.clone();
            self.draft.project = Some(id);
            self.toast("That project is already added.", true);
            return;
        }
        let name = root.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "project".into());
        let p = Project {
            id: new_id(),
            name,
            path: root,
            pinned: false,
            archived: false,
            created_at: now(),
            last_used_at: now(),
            scripts: detect_scripts_sync_hint(),
        };
        self.store.save_project(&p);
        let id = p.id.clone();
        self.projects.push(p);
        self.draft.project = Some(id.clone());
        self.load_branches(&id);
        self.starters.remove(&id);
        self.view = View::NewThread;
        self.toast("Project added.", true);
    }

    pub fn load_branches(&self, project: &str) {
        let Some(p) = self.project(project) else { return };
        let (path, id) = (p.path.clone(), project.to_string());
        self.bg(move || {
            let list = git::branches(&path).map(|b| b.into_iter().map(|b| b.name).collect()).unwrap_or_default();
            Bg::Branches { project: id, list, default: git::default_base_branch(&path) }
        });
    }

    pub fn load_starters(&mut self, project: &str) {
        if self.starters.contains_key(project) {
            return;
        }
        let Some(path) = self.project(project).map(|p| p.path.clone()) else { return };
        self.starters.insert(project.to_string(), None);
        let id = project.to_string();
        self.bg(move || Bg::Starters { project: id, list: crate::suggest::starters(&path) });
    }

    // ------------------------------------------------------------------ threads

    pub fn open_thread(&mut self, id: &str) {
        if !self.items.contains_key(id) {
            let list = self.store.load_items(id);
            self.items.insert(id.to_string(), list);
        }
        if !self.checkpoints.contains_key(id) {
            let cps = self.store.load_checkpoints(id);
            self.checkpoints.insert(id.to_string(), cps);
            let cms = self.store.load_comments(id);
            self.comments.insert(id.to_string(), cms);
            let v = self.store.load_viewed(id).into_iter().collect();
            self.viewed.insert(id.to_string(), v);
        }
        self.current = Some(id.to_string());
        self.view = View::Thread;
        self.ui.scroll_to_bottom = true;
        self.ui.selected_checkpoint = None;
        self.ui.review_file = 0;
        self.refresh_git(id);
        self.save_ui();
    }

    pub fn rename_thread(&mut self, id: &str, title: String) {
        if let Some(t) = self.thread_mut(id) {
            t.title = title;
            t.updated_at = now();
        }
        self.save_thread(id);
    }

    pub fn set_archived(&mut self, id: &str, archived: bool) {
        if let Some(t) = self.thread_mut(id) {
            t.archived = archived;
        }
        self.save_thread(id);
        if archived {
            self.sessions.remove(id);
            if self.current.as_deref() == Some(id) {
                self.current = None;
                self.view = View::NewThread;
            }
            self.toast("Thread archived. Restore it from Settings → Archive.", true);
        }
    }

    pub fn toggle_pin(&mut self, id: &str) {
        if let Some(t) = self.thread_mut(id) {
            t.pinned = !t.pinned;
        }
        self.save_thread(id);
    }

    pub fn delete_thread(&mut self, id: &str) {
        self.sessions.remove(id);
        self.threads.retain(|t| t.id != id);
        self.items.remove(id);
        self.store.delete_thread(id);
        if self.current.as_deref() == Some(id) {
            self.current = None;
            self.view = View::NewThread;
        }
        self.toast("Thread deleted. Its worktree and branch were left on disk.", true);
    }

    /// Starts one thread (or a compare group) from the New thread screen.
    pub fn start_from_draft(&mut self) {
        let prompt = self.draft.prompt.trim().to_string();
        if prompt.is_empty() {
            return;
        }
        let project = if self.draft.no_project { None } else { self.draft.project.clone() };
        if project.is_none() && !self.draft.no_project {
            self.toast("Add a project first, or start without a project.", false);
            return;
        }
        let attachments = std::mem::take(&mut self.draft.attachments);
        let runs: Vec<(ProviderKind, String)> = if self.draft.compare && self.draft.runs.len() >= 2 {
            self.draft.runs.clone()
        } else {
            vec![(self.draft.provider, self.draft.model.clone())]
        };
        let group = (runs.len() > 1).then(new_id);
        let base_branch = if self.draft.base_branch.is_empty() {
            project.as_deref().and_then(|p| self.branches.get(p)).map(|b| b.1.clone()).unwrap_or_default()
        } else {
            self.draft.base_branch.clone()
        };
        // Compare runs always isolate each run in its own worktree.
        let worktree = (self.draft.worktree || group.is_some()) && project.is_some();
        let title = title_from(&prompt);
        let mut first = None;
        for (i, (kind, model)) in runs.iter().enumerate() {
            let (dm, de) = self.default_model(*kind);
            let model = if model.is_empty() { dm } else { model.clone() };
            let effort = if self.draft.effort.is_empty() || runs.len() > 1 { de } else { self.draft.effort.clone() };
            let mut t = Thread::new(project.clone(), title.clone(), *kind, model);
            t.effort = Some(effort).filter(|e| !e.is_empty());
            t.permissions = self.settings.permissions.clone();
            t.compare_group = group.clone();
            t.run_label = group.as_ref().map(|_| ((b'A' + i as u8) as char).to_string());
            t.base_branch = Some(base_branch.clone()).filter(|b| !b.is_empty());
            t.is_worktree = worktree;
            t.state = RunState::Queued;
            t.last_activity = Some(if worktree { "Creating worktree".into() } else { "Starting".into() });
            if project.is_none() {
                let dir = crate::log::data_dir().join("scratch").join(&t.id);
                let _ = std::fs::create_dir_all(&dir);
                t.cwd = Some(dir);
            }
            let id = t.id.clone();
            self.store.save_thread(&t);
            self.threads.push(t);
            self.items.insert(id.clone(), Vec::new());
            self.checkpoints.insert(id.clone(), Vec::new());
            self.comments.insert(id.clone(), Vec::new());
            self.viewed.insert(id.clone(), HashMap::new());
            self.outbox.entry(id.clone()).or_default().push(Outgoing::Message { text: prompt.clone(), attachments: attachments.clone() });
            self.push_item(
                &id,
                ItemKind::User { text: prompt.clone(), attachments: attachments.iter().map(|p| p.display().to_string()).collect() },
            );
            first.get_or_insert(id.clone());
        }
        if let Some(p) = project.as_deref().and_then(|p| self.projects.iter_mut().find(|x| x.id == p)) {
            p.last_used_at = now();
            let p = p.clone();
            self.store.save_project(&p);
        }
        self.prepare_workspaces(group.as_deref(), first.as_deref().unwrap_or_default());
        self.draft.prompt.clear();
        match group {
            Some(g) => self.view = View::Compare(g),
            None => {
                if let Some(id) = first {
                    self.open_thread(&id);
                }
            }
        }
    }

    /// Creates worktrees (all compare runs from one resolved base sha) and resolves cwd/branch.
    fn prepare_workspaces(&mut self, group: Option<&str>, single: &str) {
        let ids: Vec<String> = match group {
            Some(g) => self.threads.iter().filter(|t| t.compare_group.as_deref() == Some(g)).map(|t| t.id.clone()).collect(),
            None => vec![single.to_string()],
        };
        let Some(t0) = self.thread(&ids[0]).cloned() else { return };
        let Some(project) = t0.project_id.as_deref().and_then(|p| self.project(p)).cloned() else {
            // No project: scratch directory already set.
            for id in ids {
                let cwd = self.thread(&id).and_then(|t| t.cwd.clone()).unwrap_or_default();
                let _ = self.bg_tx.send(Bg::WorkspaceReady { thread: id, result: Ok((cwd, None, None)) });
            }
            return;
        };
        let prefix = self.settings.branch_prefix.clone();
        let root = self.worktree_root();
        let from_remote = self.settings.worktree_from_remote;
        let jobs: Vec<(String, bool, String, Option<String>)> = ids
            .iter()
            .filter_map(|id| self.thread(id).map(|t| (id.clone(), t.is_worktree, t.title.clone(), t.run_label.clone())))
            .collect();
        let base = t0.base_branch.clone();
        self.bg(move || {
            let repo = project.path.clone();
            // Resolve the base revision once so every run starts from the identical commit.
            let base_name = base.unwrap_or_else(|| git::default_base_branch(&repo));
            let base_rev = if from_remote {
                let _ = std::process::Command::new("git").arg("-C").arg(&repo).args(["fetch", "--quiet", "origin"]).status();
                git::rev_parse(&repo, &format!("origin/{base_name}")).or_else(|_| git::rev_parse(&repo, &base_name))
            } else {
                git::rev_parse(&repo, &base_name)
            };
            let mut results = Vec::new();
            for (id, wt, title, label) in jobs {
                let r = (|| -> Result<(PathBuf, Option<String>, Option<String>), String> {
                    if !wt {
                        let head = git::head_sha(&repo).map_err(|e| e.message)?;
                        let branch = git::current_branch(&repo).map_err(|e| e.message)?;
                        return Ok((repo.clone(), branch, head));
                    }
                    let base_rev = base_rev.clone().map_err(|e| format!("Can't resolve base branch {base_name}: {}", e.message))?;
                    let mut slug = git::slugify(&title, 40);
                    if let Some(l) = &label {
                        slug.push('-');
                        slug.push_str(&l.to_ascii_lowercase());
                    }
                    let branch = git::unique_branch_name(&repo, &prefix, &slug);
                    let leaf = branch.rsplit('/').next().unwrap_or(&slug).to_string();
                    let mut path = root.join(sanitize(&project.name)).join(&leaf);
                    let mut n = 2;
                    while path.exists() {
                        path = root.join(sanitize(&project.name)).join(format!("{leaf}-{n}"));
                        n += 1;
                    }
                    git::worktree_add(&repo, &path, &branch, &base_rev).map_err(|e| format!("{}: {}", e.message, e.stderr))?;
                    Ok((path, Some(branch), Some(base_rev)))
                })();
                results.push((id, r));
            }
            // Every run gets its own result, whether or not the others succeeded.
            let mut it = results.into_iter();
            let (first_id, first_r) = it.next().expect("at least one run");
            for (id, r) in it {
                crate::app::defer_bg(Bg::WorkspaceReady { thread: id, result: r });
            }
            Bg::WorkspaceReady { thread: first_id, result: first_r }
        });
    }

    fn workspace_ready(&mut self, id: &str, result: Result<(PathBuf, Option<String>, Option<String>), String>) {
        match result {
            Ok((cwd, branch, base_rev)) => {
                if let Some(t) = self.thread_mut(id) {
                    t.cwd = Some(cwd);
                    t.branch = branch;
                    t.base_rev = base_rev;
                    t.last_activity = Some("Starting the agent".into());
                }
                self.save_thread(id);
                self.refresh_git(id);
                self.checkpoint(id, "Thread started".into());
                if self.outbox.get(id).is_some_and(|o| !o.is_empty()) {
                    self.ensure_session(id);
                } else if let Some(t) = self.thread_mut(id) {
                    t.state = RunState::Idle;
                    t.last_activity = None;
                }
            }
            Err(e) => {
                if let Some(t) = self.thread_mut(id) {
                    t.state = RunState::Failed;
                    t.last_error = Some(e.clone());
                    t.last_activity = Some("Workspace setup failed".into());
                }
                self.outbox.remove(id);
                self.save_thread(id);
                self.push_item(id, ItemKind::Error { text: format!("Could not prepare the workspace: {e}") });
            }
        }
    }

    pub fn active_count(&self) -> usize {
        self.threads.iter().filter(|t| matches!(t.state, RunState::Working | RunState::NeedsApproval)).count()
    }

    /// Spawns the provider session if needed (respecting the parallel-agents limit).
    fn ensure_session(&mut self, id: &str) {
        if self.sessions.contains_key(id) {
            self.flush_outbox(id);
            return;
        }
        if self.starting.contains(id) {
            return;
        }
        let Some(t) = self.thread(id).cloned() else { return };
        if self.demo {
            self.outbox.remove(id);
            if let Some(t) = self.thread_mut(id) {
                t.state = RunState::Idle;
            }
            self.push_item(
                id,
                ItemKind::Notice { text: "Demo mode: agents don't run. Start Kiln without --demo to use your providers.".into() },
            );
            return;
        }
        if self.active_count() >= self.settings.parallel_agents.max(1) as usize {
            if let Some(t) = self.thread_mut(id) {
                t.state = RunState::Queued;
                t.last_activity = Some("Waiting for a free agent slot".into());
            }
            return;
        }
        let Some(cwd) = self.cwd_of(&t) else { return };
        if !self.provider_enabled(t.provider) {
            self.fail_start(id, format!("{} is turned off in Settings → Providers.", t.provider.label()));
            return;
        }
        let status = self.providers.get(&t.provider).cloned();
        let exe = match status.as_ref().and_then(|s| s.executable.clone()) {
            Some(e) => e,
            None => {
                let configured = match t.provider {
                    ProviderKind::Claude => self.settings.claude_path.clone(),
                    ProviderKind::Codex => self.settings.codex_path.clone(),
                };
                match provider::find_executable(t.provider, &configured) {
                    Some(e) => e,
                    None => {
                        let hint = ProviderStatus::unknown(t.provider).install_hint().to_string();
                        self.fail_start(id, format!("{} is not installed or not on PATH. {hint}", t.provider.label()));
                        return;
                    }
                }
            }
        };
        if let Some(s) = &status
            && s.auth == provider::AuthState::SignedOut
        {
            self.fail_start(
                id,
                format!("{} is signed out. Run `{}` in a terminal, then try again.", t.provider.label(), s.login_command()),
            );
            return;
        }
        let cfg = provider::SessionConfig {
            kind: t.provider,
            exe,
            cwd,
            model: Some(t.model.clone()).filter(|m| !m.is_empty()),
            effort: t.effort.clone(),
            permissions: t.permissions.clone(),
            resume: t.native_session_id.clone(),
            thread_id: id.to_string(),
        };
        self.starting.insert(id.to_string());
        if let Some(t) = self.thread_mut(id) {
            t.state = RunState::Working;
            t.last_activity = Some(format!("Starting {}", t.provider.label()));
        }
        let (tx, ctx, tid) = (self.ev_tx.clone(), self.ctx.clone(), id.to_string());
        self.bg(move || Bg::SessionReady { thread: tid, result: provider::spawn(cfg, tx, ctx) });
    }

    fn fail_start(&mut self, id: &str, msg: String) {
        self.outbox.remove(id);
        if let Some(t) = self.thread_mut(id) {
            t.state = RunState::Failed;
            t.last_error = Some(msg.clone());
            t.last_activity = Some("Could not start".into());
        }
        self.save_thread(id);
        self.push_item(id, ItemKind::Error { text: msg });
    }

    fn flush_outbox(&mut self, id: &str) {
        let Some(list) = self.outbox.remove(id) else { return };
        for o in list {
            let Outgoing::Message { text, attachments } = o;
            self.dispatch_message(id, text, attachments, false);
        }
    }

    fn dispatch_message(&mut self, id: &str, text: String, attachments: Vec<PathBuf>, steer: bool) {
        if !steer {
            self.checkpoint(id, format!("Before: {}", provider::short(&text, 60)));
        }
        let ok = self.sessions.get(id).map(|s| s.send(Cmd::Send { text, attachments, steer })).unwrap_or(false);
        if let Some(t) = self.thread_mut(id)
            && ok
        {
            t.state = RunState::Working;
            if t.turn_started_at.is_none() {
                t.turn_started_at = Some(now());
            }
            t.last_activity = Some("Thinking".into());
            t.settled = false;
            t.updated_at = now();
        }
        self.save_thread(id);
    }

    /// Composer send: new message, steer, or queue as a follow-up.
    pub fn send(&mut self, id: &str, text: String, force_opposite: bool) {
        let text = text.trim().to_string();
        if text.is_empty() {
            return;
        }
        let attachments = self.ui.attachments.remove(id).unwrap_or_default();
        let Some(t) = self.thread(id).cloned() else { return };
        self.push_item(
            id,
            ItemKind::User { text: text.clone(), attachments: attachments.iter().map(|p| p.display().to_string()).collect() },
        );
        self.ui.scroll_to_bottom = true;
        let running = t.state.is_active() && self.sessions.contains_key(id);
        let steer_pref = (self.settings.follow_up == "Steer") != force_opposite;
        let caps = provider::capabilities(t.provider);
        if running && t.state == RunState::Working && steer_pref && caps.steer {
            self.dispatch_message(id, text, attachments, true);
            return;
        }
        if running {
            if let Some(t) = self.thread_mut(id) {
                t.queued.push(text.clone());
            }
            self.outbox.entry(id.to_string()).or_default().push(Outgoing::Message { text, attachments });
            self.save_thread(id);
            return;
        }
        if let Some(t) = self.thread_mut(id) {
            t.state = RunState::Queued;
            t.last_error = None;
        }
        self.outbox.entry(id.to_string()).or_default().push(Outgoing::Message { text, attachments });
        self.ensure_session(id);
    }

    pub fn interrupt(&mut self, id: &str) {
        if let Some(s) = self.sessions.get(id) {
            s.send(Cmd::Interrupt);
            if let Some(t) = self.thread_mut(id) {
                t.last_activity = Some("Stopping".into());
            }
        } else if let Some(t) = self.thread_mut(id)
            && t.state == RunState::Queued
        {
            t.state = RunState::Cancelled;
            self.outbox.remove(id);
        }
    }

    /// Stops a session entirely (compare "Stop" on one run).
    pub fn stop_run(&mut self, id: &str) {
        self.interrupt(id);
        self.outbox.remove(id);
        self.sessions.remove(id);
        if let Some(t) = self.thread_mut(id)
            && t.state.is_active()
        {
            t.state = RunState::Cancelled;
            t.last_activity = Some("Stopped".into());
        }
        self.approvals.remove(id);
        self.save_thread(id);
    }

    // ------------------------------------------------------------------ approvals

    /// Kiln-side policy: saved rules and risk settings, applied by answering the provider.
    pub fn auto_decision(&self, t: &Thread, a: &ApprovalRequest) -> Option<(Decision, String, Option<String>)> {
        let cmd = a.command.clone().unwrap_or_default();
        if a.kind == ApprovalKind::Command || a.kind == ApprovalKind::Network {
            for r in self.rules.iter().filter(|r| r.decision == RuleDecision::Block) {
                if (r.project_id.is_none() || r.project_id == t.project_id) && r.matches(&cmd) {
                    return Some((
                        Decision::Deny { message: Some(format!("Blocked by the user's Kiln rule \"{}\".", r.pattern)) },
                        format!("Blocked {} · your rule “{}”", provider::short(&cmd, 60), r.pattern),
                        Some(r.id.clone()),
                    ));
                }
            }
            for r in self.rules.iter().filter(|r| r.decision == RuleDecision::Allow) {
                if (r.project_id.is_none() || r.project_id == t.project_id) && r.matches(&cmd) {
                    return Some((
                        Decision::Allow,
                        format!("Ran {} · allowed by your rule “{}”", provider::short(&cmd, 60), r.pattern),
                        Some(r.id.clone()),
                    ));
                }
            }
        }
        let cwd = self.cwd_of(t).map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
        let outside = a.paths.iter().any(|p| !inside(&cwd, p));
        if self.settings.risk_outside == "Block" && outside {
            return Some((
                Decision::Deny { message: Some("The user's Kiln settings block access outside the worktree.".into()) },
                format!("Blocked: {} is outside the worktree", a.paths.join(", ")),
                None,
            ));
        }
        if self.settings.risk_network == "Block" && a.network {
            return Some((
                Decision::Deny { message: Some("The user's Kiln settings block network access.".into()) },
                format!("Blocked network access: {}", provider::short(&cmd, 60)),
                None,
            ));
        }
        if self.settings.risk_read == "Always allow" && a.read_only && !outside && !a.network && !model::has_shell_chaining(&cmd) {
            return Some((Decision::Allow, format!("Allowed read-only {} · Settings → Approvals", provider::short(&cmd, 60)), None));
        }
        if self.settings.risk_write == "Always allow" && a.kind == ApprovalKind::FileChange && !a.paths.is_empty() && !outside {
            return Some((Decision::Allow, format!("Allowed edit {} · Settings → Approvals", a.paths.join(", ")), None));
        }
        None
    }

    pub fn decide(&mut self, thread: &str, request_id: &str, decision: Decision, label: Option<String>) {
        let Some(list) = self.approvals.get_mut(thread) else { return };
        let Some(pos) = list.iter().position(|a| a.request_id == request_id) else { return };
        let a = list.remove(pos);
        let remaining = list.len();
        let kind = self.thread(thread).map(|t| t.provider);
        // Codex can't carry feedback on a decline: send it as a follow-up instead.
        let mut feedback_followup = None;
        let decision = match decision {
            Decision::Deny { message: Some(m) } if kind == Some(ProviderKind::Codex) && !m.trim().is_empty() => {
                feedback_followup = Some(m);
                Decision::Deny { message: None }
            }
            d => d,
        };
        let allowed = matches!(decision, Decision::Allow | Decision::AllowForSession);
        let ok = self
            .sessions
            .get(thread)
            .map(|s| s.send(Cmd::Approve { request_id: request_id.to_string(), decision: decision.clone() }))
            .unwrap_or(false);
        let what = a.command.clone().unwrap_or_else(|| a.title.clone());
        let label = label.unwrap_or_else(|| format!("{} {}", if allowed { "Approved:" } else { "Declined:" }, provider::short(&what, 80)));
        self.push_item(thread, ItemKind::Decision { label, allowed });
        if !ok {
            self.push_item(thread, ItemKind::Error { text: "The provider session ended before the decision could be delivered.".into() });
        }
        if let Some(t) = self.thread_mut(thread)
            && remaining == 0
            && t.state == RunState::NeedsApproval
        {
            t.state = RunState::Working;
            t.last_activity = Some(if allowed { "Continuing".into() } else { "Request declined".into() });
        }
        self.save_thread(thread);
        if let Some(m) = feedback_followup {
            self.send(thread, m, false);
        }
    }

    pub fn add_rule(&mut self, project: Option<String>, pattern: String, decision: RuleDecision) {
        let r = ApprovalRule { id: new_id(), project_id: project, pattern, decision, created_at: now(), uses: 0, builtin: false };
        self.store.save_rule(&r);
        self.rules.push(r);
    }

    pub fn remove_rule(&mut self, id: &str) {
        self.rules.retain(|r| r.id != id || r.builtin);
        self.store.delete_rule(id);
    }

    // ------------------------------------------------------------------ provider events

    fn on_event(&mut self, pe: ProviderEvent) {
        let id = pe.thread_id.clone();
        if self.thread(&id).is_none() {
            return;
        }
        match pe.ev {
            Ev::Started { native_id, model, permission_mode, auth_source } => {
                let kind = self.thread(&id).map(|t| t.provider);
                if let Some(t) = self.thread_mut(&id) {
                    if !native_id.is_empty() {
                        t.native_session_id = Some(native_id);
                    }
                    if let Some(m) = model.filter(|_| t.model.is_empty() || t.model == "default") {
                        t.last_activity = Some(format!("Using {m}"));
                    }
                    t.provider_mode = permission_mode;
                }
                if kind == Some(ProviderKind::Claude)
                    && let Some(src) = auth_source.filter(|s| s != "none" && !s.is_empty())
                {
                    self.push_item(&id, ItemKind::Notice { text: format!("Claude Code is authenticating with {src} (configured outside Kiln), not your claude.ai login. Usage may be billed to that key.") });
                }
                self.save_thread(&id);
            }
            Ev::TurnStarted => {
                if let Some(t) = self.thread_mut(&id) {
                    t.state = RunState::Working;
                    t.turn_started_at.get_or_insert(now());
                }
            }
            Ev::TextDelta { id: pid, text } => self.append_live(&id, &pid, &text, false),
            Ev::Text { id: pid, text } => {
                let idx = self.live_index(&id, &pid, false);
                if let Some(ItemKind::Assistant { text: t }) = self.items.get_mut(&id).and_then(|l| l.get_mut(idx)).map(|i| &mut i.kind) {
                    *t = text;
                }
                self.persist_item(&id, idx);
                if let Some(t) = self.thread_mut(&id) {
                    t.last_activity = Some("Writing a reply".into());
                }
            }
            Ev::ReasoningDelta { id: pid, text } => {
                self.append_live(&id, &pid, &text, true);
                if let Some(t) = self.thread_mut(&id) {
                    t.last_activity = Some("Thinking".into());
                }
            }
            Ev::ToolStarted { id: pid, kind, label, detail, paths } => {
                let idx = self.push_item(
                    &id,
                    ItemKind::Tool {
                        tool_id: pid.clone(),
                        kind,
                        label: label.clone(),
                        detail,
                        output: None,
                        status: ToolStatus::Running,
                        paths,
                    },
                );
                self.live.entry(id.clone()).or_default().insert(format!("tool:{pid}"), idx);
                if let Some(t) = self.thread_mut(&id) {
                    t.last_activity = Some(label);
                }
            }
            Ev::ToolOutput { id: pid, text } => {
                if let Some(&idx) = self.live.get(&id).and_then(|m| m.get(&format!("tool:{pid}")))
                    && let Some(ItemKind::Tool { output, .. }) = self.items.get_mut(&id).and_then(|l| l.get_mut(idx)).map(|i| &mut i.kind)
                {
                    let o = output.get_or_insert_with(String::new);
                    if o.len() < 16_000 {
                        o.push_str(&text);
                    }
                }
            }
            Ev::ToolDone { id: pid, status: st, output: out } => {
                if let Some(&idx) = self.live.get(&id).and_then(|m| m.get(&format!("tool:{pid}"))) {
                    if let Some(ItemKind::Tool { output, status, .. }) =
                        self.items.get_mut(&id).and_then(|l| l.get_mut(idx)).map(|i| &mut i.kind)
                    {
                        *status = st;
                        if out.as_deref().is_some_and(|o| !o.is_empty()) {
                            *output = out;
                        }
                    }
                    self.persist_item(&id, idx);
                }
            }
            Ev::Approval(a) => {
                let Some(t) = self.thread(&id).cloned() else { return };
                if let Some((decision, label, rule)) = self.auto_decision(&t, &a) {
                    if let Some(rid) = rule
                        && let Some(r) = self.rules.iter_mut().find(|r| r.id == rid)
                    {
                        r.uses += 1;
                        let r = r.clone();
                        self.store.save_rule(&r);
                    }
                    self.approvals.entry(id.clone()).or_default().push(a.clone());
                    self.decide(&id, &a.request_id, decision, Some(label));
                    return;
                }
                self.approvals.entry(id.clone()).or_default().push(a.clone());
                if let Some(t) = self.thread_mut(&id) {
                    t.state = RunState::NeedsApproval;
                    t.last_activity = Some(a.title.clone());
                }
                self.save_thread(&id);
                let title = t.title.clone();
                self.notify(&format!("Needs approval · {title}"), &a.command.clone().unwrap_or(a.title.clone()), &id);
            }
            Ev::ApprovalGone { request_id } => {
                if let Some(list) = self.approvals.get_mut(&id) {
                    list.retain(|a| a.request_id != request_id);
                    if list.is_empty()
                        && let Some(t) = self.thread_mut(&id)
                        && t.state == RunState::NeedsApproval
                    {
                        t.state = RunState::Working;
                    }
                }
            }
            Ev::Plan(steps) => {
                if let Some(t) = self.thread_mut(&id) {
                    t.plan = steps;
                }
                self.save_thread(&id);
            }
            Ev::Usage { context_tokens, context_window, input, output, cached, cost_usd } => {
                let kind = self.thread(&id).map(|t| t.provider);
                if let Some(t) = self.thread_mut(&id) {
                    let u = &mut t.usage;
                    if context_tokens.is_some() {
                        u.context_tokens = context_tokens;
                    }
                    if context_window.is_some() {
                        u.context_window = context_window;
                    }
                    if kind == Some(ProviderKind::Codex) {
                        // Codex reports cumulative thread totals.
                        u.total_input = input;
                        u.total_output = output;
                        u.cached_input = cached;
                    } else {
                        u.total_input += input;
                        u.total_output += output;
                        u.cached_input += cached;
                    }
                    if let Some(c) = cost_usd {
                        u.est_cost_usd = Some(u.est_cost_usd.unwrap_or(0.0) + c);
                    }
                }
                self.save_thread(&id);
            }
            Ev::RateLimit(r) => {
                if let Some(kind) = self.thread(&id).map(|t| t.provider)
                    && let Some(st) = self.providers.get_mut(&kind)
                {
                    st.rate_limits.retain(|x| x.label != r.label);
                    st.rate_limits.push(r);
                }
            }
            Ev::Models(m) => {
                if let Some(kind) = self.thread(&id).map(|t| t.provider)
                    && let Some(st) = self.providers.get_mut(&kind)
                    && !m.is_empty()
                {
                    st.models = m;
                }
            }
            Ev::FilesTouched => self.schedule_git(&id, 0.6),
            Ev::TurnDone { outcome, error } => self.turn_done(&id, outcome, error),
            Ev::Error { message, will_retry } => {
                if will_retry {
                    if let Some(t) = self.thread_mut(&id) {
                        t.last_activity = Some(format!("Retrying: {}", provider::short(&message, 60)));
                    }
                } else {
                    self.push_item(&id, ItemKind::Error { text: message.clone() });
                    self.auth_hint(&id, &message);
                    if let Some(t) = self.thread_mut(&id) {
                        t.last_error = Some(message);
                    }
                }
            }
            Ev::Log(l) => crate::log::info(&format!("[{id}] {l}")),
            Ev::Exited { code, stderr_tail } => {
                self.sessions.remove(&id);
                self.approvals.remove(&id);
                let active = self.thread(&id).map(|t| t.state.is_active()).unwrap_or(false);
                if active {
                    let tail: String =
                        stderr_tail.lines().rev().take(6).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n");
                    let msg = format!(
                        "The provider process exited unexpectedly{}.{}",
                        code.map(|c| format!(" (code {c})")).unwrap_or_default(),
                        if tail.trim().is_empty() { String::new() } else { format!("\n\n{}", crate::log::redact(&tail)) }
                    );
                    self.push_item(&id, ItemKind::Error { text: msg.clone() });
                    if let Some(t) = self.thread_mut(&id) {
                        t.state = RunState::Failed;
                        t.last_error = Some(msg);
                        t.turn_started_at = None;
                    }
                    self.save_thread(&id);
                }
                self.start_queued();
            }
        }
    }

    fn live_index(&mut self, thread: &str, pid: &str, reasoning: bool) -> usize {
        let key = format!("{}{pid}", if reasoning { "r:" } else { "a:" });
        if let Some(&i) = self.live.get(thread).and_then(|m| m.get(&key)) {
            return i;
        }
        let idx = self.push_item(
            thread,
            if reasoning { ItemKind::Reasoning { text: String::new() } } else { ItemKind::Assistant { text: String::new() } },
        );
        self.live.entry(thread.to_string()).or_default().insert(key, idx);
        idx
    }

    fn append_live(&mut self, thread: &str, pid: &str, delta: &str, reasoning: bool) {
        let idx = self.live_index(thread, pid, reasoning);
        if let Some(it) = self.items.get_mut(thread).and_then(|l| l.get_mut(idx)) {
            match &mut it.kind {
                ItemKind::Assistant { text } | ItemKind::Reasoning { text } => text.push_str(delta),
                _ => {}
            }
        }
    }

    fn turn_done(&mut self, id: &str, outcome: TurnOutcome, error: Option<String>) {
        // Persist streamed items that never got a final record.
        if let Some(map) = self.live.remove(id) {
            for idx in map.values() {
                self.persist_item(id, *idx);
            }
        }
        let started = self.thread(id).and_then(|t| t.turn_started_at);
        let secs = started.map(|s| now() - s).unwrap_or(0);
        let gs = self.git.get(id).map(|g| g.totals()).unwrap_or((0, 0, 0));
        let queued = !self.outbox.get(id).map(|o| o.is_empty()).unwrap_or(true);
        let title = self.thread(id).map(|t| t.title.clone()).unwrap_or_default();
        if let Some(t) = self.thread_mut(id) {
            t.worked_secs += secs;
            t.turn_started_at = None;
            t.updated_at = now();
            t.queued.clear();
            t.state = match outcome {
                TurnOutcome::Completed if gs.0 > 0 => RunState::ReadyToReview,
                TurnOutcome::Completed => RunState::Completed,
                TurnOutcome::Interrupted => RunState::Cancelled,
                TurnOutcome::Failed => RunState::Failed,
            };
            t.last_activity = Some(match outcome {
                TurnOutcome::Completed => "Finished".into(),
                TurnOutcome::Interrupted => "Stopped".into(),
                TurnOutcome::Failed => "Failed".into(),
            });
            if outcome == TurnOutcome::Failed {
                t.last_error = error.clone();
            }
            if t.plan.iter().all(|s| s.status == StepStatus::Completed) && outcome == TurnOutcome::Completed {
                // keep plan for display
            }
        }
        self.push_item(id, ItemKind::TurnEnd { secs, files: gs.0, additions: gs.1, deletions: gs.2 });
        if let (TurnOutcome::Failed, Some(e)) = (&outcome, &error) {
            self.push_item(id, ItemKind::Error { text: e.clone() });
            self.auth_hint(id, e);
        }
        self.save_thread(id);
        self.refresh_git(id);
        if self.settings.proactive_changes && (gs.0 >= 3 || gs.1 + gs.2 >= 50) && self.current.as_deref() == Some(id) {
            self.ui.right_open = true;
            self.ui.right_tab = Some(RightTab::Diff);
        }
        match outcome {
            TurnOutcome::Completed => self.notify(&format!("Finished · {title}"), "The agent finished its turn.", id),
            TurnOutcome::Failed => self.notify(&format!("Failed · {title}"), error.as_deref().unwrap_or("The turn failed."), id),
            TurnOutcome::Interrupted => {}
        }
        if queued {
            self.flush_outbox(id);
        }
        self.start_queued();
    }

    /// A provider auth failure gets a concrete recovery step and a fresh (read-only) status check.
    fn auth_hint(&mut self, id: &str, text: &str) {
        let t = text.to_ascii_lowercase();
        if !(t.contains("authenticat") || t.contains("401") || t.contains("not logged in") || t.contains("token has expired")) {
            return;
        }
        let Some(kind) = self.thread(id).map(|t| t.provider) else { return };
        let cmd = ProviderStatus::unknown(kind).login_command();
        self.push_item(id, ItemKind::Notice { text: format!("{} couldn't authenticate. Kiln doesn't sign in for you: run `{cmd}` in a terminal (Settings → Providers can open one), then send your message again.", kind.label()) });
        self.refresh_providers();
    }

    fn start_queued(&mut self) {
        let waiting: Vec<String> = self
            .threads
            .iter()
            .filter(|t| {
                t.state == RunState::Queued && !self.sessions.contains_key(&t.id) && !self.starting.contains(&t.id) && t.cwd.is_some()
            })
            .map(|t| t.id.clone())
            .collect();
        for id in waiting {
            if self.active_count() >= self.settings.parallel_agents.max(1) as usize {
                break;
            }
            if self.outbox.contains_key(&id) {
                self.ensure_session(&id);
            }
        }
    }

    pub fn notify(&mut self, title: &str, body: &str, thread: &str) {
        let focused_here = self.focused && self.current.as_deref() == Some(thread);
        if self.focused {
            if self.settings.in_app_toasts && !focused_here {
                self.toast(title.to_string(), true);
            }
            return;
        }
        if self.settings.notify_popup() || self.settings.notify_sound() {
            let (t, b, sound) = (title.to_string(), provider::short(body, 160), self.settings.notify_sound());
            let popup = self.settings.notify_popup();
            std::thread::spawn(move || {
                let mut n = notify_rust::Notification::new();
                n.summary(&t).body(&b).appname("Kiln Code");
                if sound {
                    n.sound_name("Default");
                }
                if popup {
                    let _ = n.show();
                }
            });
            self.ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
        }
    }

    // ------------------------------------------------------------------ git

    pub fn schedule_git(&mut self, id: &str, delay: f64) {
        let t = self.time() + delay;
        let g = self.git.entry(id.to_string()).or_default();
        if g.refresh_at.is_none() {
            g.refresh_at = Some(t);
        }
    }

    pub fn refresh_git(&mut self, id: &str) {
        let Some(t) = self.thread(id).cloned() else { return };
        let Some(cwd) = self.cwd_of(&t) else { return };
        let g = self.git.entry(id.to_string()).or_default();
        if g.loading {
            g.refresh_at = Some(0.0);
            return;
        }
        g.loading = true;
        g.refresh_at = None;
        let tid = id.to_string();
        let base = t.base_rev.clone();
        self.bg(move || {
            let r = match &base {
                Some(b) => git::changes_since(&cwd, b),
                None => git::status(&cwd),
            };
            Bg::Status { thread: tid, result: r.map_err(|e| e.message) }
        });
    }

    pub fn load_diff(&mut self, id: &str) {
        let Some(t) = self.thread(id).cloned() else { return };
        let Some(cwd) = self.cwd_of(&t) else { return };
        let g = self.git.entry(id.to_string()).or_default();
        if g.diff_loading {
            return;
        }
        g.diff_loading = true;
        let base = g.diff_base.as_ref().map(|b| b.0.clone()).or(t.base_rev.clone()).unwrap_or_else(|| "HEAD".into());
        let dark = theme::p().dark;
        let tid = id.to_string();
        self.bg(move || {
            let r = git::diff_worktree(&cwd, &base).map(|files| {
                let spans = files
                    .iter()
                    .map(|f| {
                        let lines: Vec<&str> = f.hunks.iter().flat_map(|h| h.lines.iter().map(|l| l.text.as_str())).collect();
                        if f.binary || lines.len() > 20_000 {
                            vec![Vec::new(); lines.len()]
                        } else {
                            crate::highlight::lines(&f.path, &lines, dark)
                        }
                    })
                    .collect();
                let fingerprints = files.iter().map(fingerprint).collect();
                DiffDoc { files, spans, fingerprints }
            });
            Bg::Diff { thread: tid, result: r.map_err(|e| e.message) }
        });
    }

    pub fn checkpoint(&mut self, id: &str, title: String) {
        if !self.settings.checkpoints {
            return;
        }
        let Some(t) = self.thread(id).cloned() else { return };
        let Some(cwd) = self.cwd_of(&t) else { return };
        if t.project_id.is_none() && !cwd.join(".git").exists() {
            return;
        }
        let n = self.checkpoints.get(id).map(|c| c.len()).unwrap_or(0);
        let tid = id.to_string();
        let base = t.base_rev.clone();
        self.bg(move || {
            let r = git::checkpoint_create(&cwd, &format!("{tid}/{n}"), &title).map(|sha| {
                let ch = base.as_deref().map(|b| git::diff_revs(&cwd, b, &sha).unwrap_or_default()).unwrap_or_default();
                let (a, d) = ch.iter().fold((0, 0), |(a, d), f| (a + f.additions, d + f.deletions));
                Checkpoint {
                    id: new_id(),
                    thread_id: tid.clone(),
                    sha,
                    title,
                    detail: format!("{} file{} changed", ch.len(), if ch.len() == 1 { "" } else { "s" }),
                    additions: a,
                    deletions: d,
                    created_at: now(),
                }
            });
            Bg::Checkpoint { thread: tid, result: r.map_err(|e| e.message) }
        });
    }

    pub fn preview_restore(&mut self, id: &str, cp: Checkpoint) {
        let Some(cwd) = self.thread(id).and_then(|t| self.cwd_of(t)) else { return };
        let tid = id.to_string();
        self.bg(move || {
            let r = git::checkpoint_changes(&cwd, &cp.sha).map_err(|e| e.message);
            Bg::RestorePreview { thread: tid, cp, result: r }
        });
    }

    pub fn restore(&mut self, id: &str, cp: Checkpoint) {
        let Some(cwd) = self.thread(id).and_then(|t| self.cwd_of(t)) else { return };
        let tid = id.to_string();
        let n = self.checkpoints.get(id).map(|c| c.len()).unwrap_or(0);
        self.bg(move || {
            let r = git::checkpoint_restore(&cwd, &cp.sha, &format!("{tid}/{n}-safety"));
            match r {
                Ok(safety) => Bg::Done {
                    thread: Some(tid),
                    ok: true,
                    text: format!(
                        "Restored files to “{}”. The previous state was saved first ({}).",
                        cp.title,
                        &safety[..safety.len().min(7)]
                    ),
                    kind: DoneKind::Restore,
                },
                Err(e) => {
                    Bg::Done { thread: Some(tid), ok: false, text: format!("Restore failed: {}", e.message), kind: DoneKind::Restore }
                }
            }
        });
    }

    pub fn commit_and_push(&mut self, id: &str, message: String, push: bool) {
        let Some(t) = self.thread(id).cloned() else { return };
        let Some(cwd) = self.cwd_of(&t) else { return };
        if self.settings.require_viewed && !self.all_viewed(id) {
            self.toast("Mark every changed file as Viewed first (Settings → Diffs & pull requests).", false);
            return;
        }
        self.ui.busy.insert(format!("commit:{id}"));
        let tid = id.to_string();
        let branch = t.branch.clone();
        self.bg(move || {
            let dirty = git::is_dirty(&cwd).unwrap_or(false);
            if dirty && let Err(e) = git::commit_all(&cwd, &message) {
                return Bg::Done {
                    thread: Some(tid),
                    ok: false,
                    text: format!("Commit failed: {} {}", e.message, e.stderr),
                    kind: DoneKind::Commit,
                };
            }
            if !push {
                return Bg::Done { thread: Some(tid), ok: true, text: "Committed.".into(), kind: DoneKind::Commit };
            }
            let Some(b) = branch.or_else(|| git::current_branch(&cwd).ok().flatten()) else {
                return Bg::Done { thread: Some(tid), ok: false, text: "Can't push a detached HEAD.".into(), kind: DoneKind::Push };
            };
            match git::push(&cwd, &b) {
                Ok(_) => Bg::Done {
                    thread: Some(tid),
                    ok: true,
                    text: format!("{}Pushed {b} to origin.", if dirty { "Committed. " } else { "" }),
                    kind: DoneKind::Push,
                },
                Err(e) => Bg::Done {
                    thread: Some(tid),
                    ok: false,
                    text: format!("Push failed: {} {}", e.message, e.stderr.trim()),
                    kind: DoneKind::Push,
                },
            }
        });
    }

    pub fn open_pr(&mut self, id: &str, title: String, body: String, base: String, draft: bool) {
        let Some(t) = self.thread(id).cloned() else { return };
        let Some(cwd) = self.cwd_of(&t) else { return };
        if !matches!(self.gh.as_ref().map(|g| &g.0), Some(git::GhAuth::SignedIn { .. })) {
            self.toast("Pull requests need the GitHub CLI signed in. Run `gh auth login` in a terminal.", false);
            return;
        }
        let Some(branch) = t.branch.clone() else {
            self.toast("This thread has no branch to open a pull request from.", false);
            return;
        };
        self.ui.busy.insert(format!("pr:{id}"));
        let tid = id.to_string();
        self.bg(move || {
            if let Err(e) = git::push(&cwd, &branch) {
                return Bg::Done {
                    thread: Some(tid),
                    ok: false,
                    text: format!("Push failed: {} {}", e.message, e.stderr.trim()),
                    kind: DoneKind::Pr,
                };
            }
            match git::gh_pr_create(&cwd, &title, &body, &base, &branch, draft) {
                Ok(url) => Bg::Done { thread: Some(tid), ok: true, text: url, kind: DoneKind::Pr },
                Err(e) => Bg::Done {
                    thread: Some(tid),
                    ok: false,
                    text: format!("Couldn't open the pull request: {} {}", e.message, e.stderr.trim()),
                    kind: DoneKind::Pr,
                },
            }
        });
    }

    pub fn merge_pr(&mut self, id: &str, url: String) {
        let method = match self.settings.merge_method.as_str() {
            "Merge" => "--merge",
            "Rebase and merge" => "--rebase",
            _ => "--squash",
        };
        let Some(cwd) = self.thread(id).and_then(|t| self.cwd_of(t)) else { return };
        let tid = id.to_string();
        self.bg(move || {
            let mut cmd = std::process::Command::new("gh");
            cmd.current_dir(&cwd).args(["pr", "merge", &url, method]).env("GH_PROMPT_DISABLED", "1");
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x0800_0000);
            }
            match provider::process::output_with_timeout(cmd, std::time::Duration::from_secs(120)) {
                Ok(o) if o.status.success() => {
                    Bg::Done { thread: Some(tid), ok: true, text: "Merged the pull request.".into(), kind: DoneKind::Merge }
                }
                Ok(o) => Bg::Done {
                    thread: Some(tid),
                    ok: false,
                    text: format!("Merge failed: {}", String::from_utf8_lossy(&o.stderr).trim()),
                    kind: DoneKind::Merge,
                },
                Err(e) => Bg::Done { thread: Some(tid), ok: false, text: format!("Merge failed: {e}"), kind: DoneKind::Merge },
            }
        });
    }

    pub fn revert_hunk(&mut self, id: &str, file: usize, hunk: usize) {
        let Some(doc) = self.git.get(id).and_then(|g| g.diff.clone()) else { return };
        let Some(f) = doc.files.get(file).cloned() else { return };
        let Some(cwd) = self.thread(id).and_then(|t| self.cwd_of(t)) else { return };
        let tid = id.to_string();
        self.bg(move || match git::revert_hunk(&cwd, &f, hunk) {
            Ok(()) => Bg::Done { thread: Some(tid), ok: true, text: format!("Reverted one hunk in {}.", f.path), kind: DoneKind::Revert },
            Err(e) => Bg::Done { thread: Some(tid), ok: false, text: format!("Couldn't revert: {}", e.message), kind: DoneKind::Revert },
        });
    }

    pub fn run_tests(&mut self, id: &str) {
        let Some(t) = self.thread(id).cloned() else { return };
        let Some(cwd) = self.cwd_of(&t) else { return };
        let project_cmd = t
            .project_id
            .as_deref()
            .and_then(|p| self.project(p))
            .and_then(|p| p.scripts.iter().find(|(n, _)| n == "test").map(|(_, c)| c.clone()));
        let Some(cmd) = project_cmd.or_else(|| crate::suggest::test_command(&cwd)) else {
            self.toast("No test command found. Add a “test” action in Settings → Projects.", false);
            return;
        };
        self.ui.busy.insert(format!("tests:{id}"));
        if let Some(t) = self.thread_mut(id) {
            t.test_summary = Some(format!("Running {cmd}…"));
            t.test_ok = None;
        }
        let tid = id.to_string();
        self.bg(move || {
            let (ok, summary) = crate::suggest::run_tests(&cwd, &cmd);
            Bg::Tests { thread: tid, ok, summary }
        });
    }

    pub fn remove_worktree(&mut self, id: &str) {
        let Some(t) = self.thread(id).cloned() else { return };
        let (Some(cwd), Some(repo)) = (t.cwd.clone(), t.project_id.as_deref().and_then(|p| self.project(p)).map(|p| p.path.clone())) else {
            return;
        };
        if !t.is_worktree {
            return;
        }
        self.sessions.remove(id);
        let tid = id.to_string();
        self.bg(move || match git::worktree_remove(&repo, &cwd) {
            Ok(()) => {
                Bg::Done { thread: Some(tid), ok: true, text: "Removed the worktree. The branch is kept.".into(), kind: DoneKind::Remove }
            }
            Err(e) => Bg::Done { thread: Some(tid), ok: false, text: format!("Kept the worktree: {}", e.message), kind: DoneKind::Remove },
        });
    }

    pub fn scan_worktrees(&mut self) {
        let projects: Vec<(String, PathBuf)> = self.projects.iter().map(|p| (p.name.clone(), p.path.clone())).collect();
        let root = self.worktree_root();
        self.bg(move || {
            let mut out = Vec::new();
            for (name, path) in projects {
                for w in git::worktree_list(&path).unwrap_or_default() {
                    if w.is_main || !w.path.starts_with(&root) {
                        continue;
                    }
                    let dirty = git::is_dirty(&w.path).unwrap_or(true);
                    out.push((w, dirty, name.clone()));
                }
            }
            Bg::Worktrees(out)
        });
    }

    pub fn cleanup_worktrees(&mut self, items: Vec<(PathBuf, String)>) {
        let projects: Vec<(String, PathBuf)> = self.projects.iter().map(|p| (p.name.clone(), p.path.clone())).collect();
        self.bg(move || {
            let mut removed = 0;
            let mut kept = Vec::new();
            for (path, project) in items {
                let Some(repo) = projects.iter().find(|p| p.0 == project).map(|p| p.1.clone()) else { continue };
                match git::worktree_remove(&repo, &path) {
                    Ok(()) => removed += 1,
                    Err(e) => kept.push(format!("{} ({})", path.display(), e.message)),
                }
            }
            let text = if kept.is_empty() {
                format!("Removed {removed} worktree(s).")
            } else {
                format!("Removed {removed}; kept {}", kept.join(", "))
            };
            Bg::Done { thread: None, ok: kept.is_empty(), text, kind: DoneKind::Other }
        });
        self.ui.cleanup_preview = None;
    }

    pub fn all_viewed(&self, id: &str) -> bool {
        let Some(doc) = self.git.get(id).and_then(|g| g.diff.clone()) else { return false };
        let v = self.viewed.get(id);
        doc.files.iter().zip(&doc.fingerprints).all(|(f, fp)| v.and_then(|m| m.get(&f.path)) == Some(fp))
    }

    pub fn set_viewed(&mut self, id: &str, path: &str, fp: Option<String>) {
        let m = self.viewed.entry(id.to_string()).or_default();
        match &fp {
            Some(f) => {
                m.insert(path.to_string(), f.clone());
            }
            None => {
                m.remove(path);
            }
        }
        self.store.set_viewed(id, path, fp);
    }

    pub fn add_comment(&mut self, id: &str, path: &str, line: u32, old_side: bool, rev: String, text: String) {
        let c = ReviewComment {
            id: new_id(),
            thread_id: id.into(),
            path: path.into(),
            line,
            old_side,
            rev,
            text,
            sent: false,
            resolved: false,
            created_at: now(),
        };
        self.store.save_comment(&c);
        self.comments.entry(id.to_string()).or_default().push(c);
    }

    /// Sends a review comment to the agent as a follow-up message anchored to file and line.
    pub fn send_comment(&mut self, id: &str, comment_id: &str) {
        let Some(c) = self.comments.get_mut(id).and_then(|l| l.iter_mut().find(|c| c.id == comment_id)) else { return };
        c.sent = true;
        let c = c.clone();
        self.store.save_comment(&c);
        let code = self.git.get(id).and_then(|g| g.diff.clone()).and_then(|d| {
            d.files.iter().find(|f| f.path == c.path).and_then(|f| {
                f.hunks
                    .iter()
                    .flat_map(|h| h.lines.iter())
                    .find(|l| crate::ui::diff::anchor_of(l) == Some((c.line, c.old_side)))
                    .map(|l| l.text.clone())
            })
        });
        let msg = format!(
            "Review comment on `{}` {} {}:{}\n\n{}",
            c.path,
            if c.old_side { "deleted line (old numbering)" } else { "line" },
            c.line,
            code.map(|c| format!("\n```\n{c}\n```")).unwrap_or_default(),
            c.text
        );
        self.send(id, msg, false);
        self.toast("Sent to the agent.", true);
    }

    pub fn resolve_comment(&mut self, id: &str, comment_id: &str) {
        if let Some(c) = self.comments.get_mut(id).and_then(|l| l.iter_mut().find(|c| c.id == comment_id)) {
            c.resolved = true;
            let c = c.clone();
            self.store.save_comment(&c);
        }
    }

    /// New thread in a fresh worktree whose files start from a checkpoint. It's a new provider
    /// session: the agent does not inherit the earlier conversation.
    pub fn fork_from_checkpoint(&mut self, id: &str, cp: &Checkpoint) {
        let Some(src) = self.thread(id).cloned() else { return };
        if src.project_id.is_none() {
            self.toast("Forking needs a project repository.", false);
            return;
        }
        let mut t = Thread::new(src.project_id.clone(), format!("Fork · {}", src.title), src.provider, src.model.clone());
        t.effort = src.effort.clone();
        t.permissions = src.permissions.clone();
        t.base_branch = Some(cp.sha.clone());
        t.is_worktree = true;
        t.state = RunState::Queued;
        t.last_activity = Some("Creating worktree".into());
        let nid = t.id.clone();
        self.store.save_thread(&t);
        self.threads.push(t);
        self.items.insert(nid.clone(), Vec::new());
        self.checkpoints.insert(nid.clone(), Vec::new());
        self.push_item(&nid, ItemKind::Notice { text: format!("Forked from the checkpoint “{}” of “{}”. The worktree starts from that checkpoint's files. This is a new agent session: it doesn't have the earlier conversation, so describe what to try next.", cp.title, src.title) });
        self.prepare_workspaces(None, &nid);
        self.open_thread(&nid);
    }

    pub fn keep_run(&mut self, id: &str) {
        let Some(group) = self.thread(id).and_then(|t| t.compare_group.clone()) else { return };
        for t in self.threads.iter_mut().filter(|t| t.compare_group.as_deref() == Some(group.as_str())) {
            t.kept = t.id == id;
        }
        let ids: Vec<String> =
            self.threads.iter().filter(|t| t.compare_group.as_deref() == Some(group.as_str())).map(|t| t.id.clone()).collect();
        for i in ids {
            self.save_thread(&i);
        }
        self.toast("Kept this run. The other runs and their worktrees are untouched; archive or clean them up when you're ready.", true);
    }

    pub fn open_terminal(&mut self, cwd: PathBuf, run: Option<String>) {
        match TerminalSession::spawn(&cwd, None, self.ctx.clone()) {
            Ok(t) => {
                if let Some(cmd) = run {
                    t.write_input(format!("{cmd}\r").as_bytes());
                }
                let n = self.terminals.iter().filter(|(k, _)| Path::new(k) == cwd).count() + 1;
                let _ = n;
                self.terminals.push((cwd.to_string_lossy().to_string(), t));
                self.ui.term_active = self.terminals.len() - 1;
                self.ui.terminal_open = true;
            }
            Err(e) => self.toast(format!("Couldn't start a terminal: {e}"), false),
        }
    }

    // ------------------------------------------------------------------ per-frame

    pub fn pump(&mut self) {
        while let Ok(ev) = self.ev_rx.try_recv() {
            self.on_event(ev);
        }
        while let Some(b) = take_deferred() {
            self.on_bg(b);
        }
        while let Ok(b) = self.bg_rx.try_recv() {
            self.on_bg(b);
        }
        let now_t = self.time();
        let due: Vec<String> =
            self.git.iter().filter(|(_, g)| g.refresh_at.is_some_and(|t| t <= now_t) && !g.loading).map(|(k, _)| k.clone()).collect();
        for id in due {
            self.refresh_git(&id);
        }
        if let Some(next) = self.git.values().filter_map(|g| g.refresh_at).reduce(f64::min) {
            self.ctx.request_repaint_after(std::time::Duration::from_secs_f64((next - now_t).max(0.05)));
        }
        if self.health_next > 0.0 && now_t >= self.health_next && self.settings.health_interval_secs > 0 {
            self.refresh_providers();
        }
        if self.settings.health_interval_secs > 0 {
            self.ctx.request_repaint_after(std::time::Duration::from_secs_f64((self.health_next - now_t).max(1.0)));
        }
        self.toasts.retain(|t| t.until > now_t);
        if let Some(t) = self.toasts.iter().map(|t| t.until).reduce(f64::min) {
            self.ctx.request_repaint_after(std::time::Duration::from_secs_f64((t - now_t).max(0.05)));
        }
        // Live elapsed timers only tick while something is running.
        if self.threads.iter().any(|t| t.state == RunState::Working) {
            self.ctx.request_repaint_after(std::time::Duration::from_secs(1));
        }
        self.terminals.retain(|(_, t)| t.is_alive() || t.exit_status().is_some());
        if self.settings != self.saved_settings {
            self.store.save_settings(&self.settings);
            if self.settings.ui_font != self.saved_settings.ui_font || self.settings.mono_font != self.saved_settings.mono_font {
                theme::install_fonts(&self.ctx, &self.settings.ui_font, &self.settings.mono_font);
            }
            if self.settings.claude_path != self.saved_settings.claude_path || self.settings.codex_path != self.saved_settings.codex_path {
                self.refresh_providers();
            }
            self.saved_settings = self.settings.clone();
        }
    }

    fn on_bg(&mut self, b: Bg) {
        match b {
            Bg::Health(st) => {
                let kind = st.kind;
                let prev_rl = self.providers.get(&kind).map(|p| p.rate_limits.clone()).unwrap_or_default();
                let mut st = st;
                if st.rate_limits.is_empty() {
                    st.rate_limits = prev_rl;
                }
                self.providers.insert(kind, st);
            }
            Bg::Gh(auth, ver, gitv) => {
                self.gh = Some((auth, ver));
                self.git_version = gitv;
            }
            Bg::Status { thread, result } => {
                let g = self.git.entry(thread.clone()).or_default();
                g.loading = false;
                g.loaded_at = self.ctx.input(|i| i.time);
                match result {
                    Ok(ch) => {
                        g.changes = ch;
                        g.error = None;
                    }
                    Err(e) => g.error = Some(e),
                }
                let (files, a, d) = g.totals();
                let had_diff = g.diff.is_some();
                if g.refresh_at == Some(0.0) {
                    g.refresh_at = Some(self.ctx.input(|i| i.time));
                }
                if let Some(t) = self.thread_mut(&thread) {
                    // A finished turn with changes is ready to review; the status usually lands
                    // just after the turn ends, so promote here rather than at TurnDone.
                    let promote = t.state == RunState::Completed && files > 0 && t.pr_url.is_none() && !t.settled;
                    if promote {
                        t.state = RunState::ReadyToReview;
                    }
                    if promote || (t.files_changed, t.additions, t.deletions) != (files, a, d) {
                        t.files_changed = files;
                        t.additions = a;
                        t.deletions = d;
                        self.save_thread(&thread);
                    }
                }
                if had_diff || self.ui.right_open || self.view == View::Review {
                    if let Some(g) = self.git.get_mut(&thread) {
                        g.diff_loading = false;
                    }
                    self.load_diff(&thread);
                }
            }
            Bg::Diff { thread, result } => {
                let g = self.git.entry(thread).or_default();
                g.diff_loading = false;
                match result {
                    Ok(d) => g.diff = Some(Arc::new(d)),
                    Err(e) => g.error = Some(e),
                }
            }
            Bg::SessionReady { thread, result } => {
                self.starting.remove(&thread);
                match result {
                    Ok(h) => {
                        self.sessions.insert(thread.clone(), h);
                        self.flush_outbox(&thread);
                    }
                    Err(e) => self.fail_start(&thread, format!("Couldn't start the provider: {e}")),
                }
            }
            Bg::WorkspaceReady { thread, result } => self.workspace_ready(&thread, result),
            Bg::Branches { project, list, default } => {
                self.branches.insert(project, (list, default));
            }
            Bg::Starters { project, list } => {
                self.starters.insert(project, Some(list));
            }
            Bg::Checkpoint { thread, result } => match result {
                Ok(cp) => {
                    self.store.save_checkpoint(&cp);
                    self.checkpoints.entry(thread).or_default().push(cp);
                }
                Err(e) => crate::log::error(&format!("checkpoint failed: {e}")),
            },
            Bg::RestorePreview { thread, cp, result } => match result {
                Ok(changes) => self.confirm = Some(Confirm::RestoreCheckpoint { thread, cp, changes }),
                Err(e) => self.toast(format!("Couldn't preview the restore: {e}"), false),
            },
            Bg::Done { thread, ok, text, kind } => {
                if let Some(t) = &thread {
                    for k in ["commit", "pr", "tests"] {
                        self.ui.busy.remove(&format!("{k}:{t}"));
                    }
                }
                match (kind, ok, &thread) {
                    (DoneKind::Pr, true, Some(t)) => {
                        let tid = t.clone();
                        if let Some(th) = self.thread_mut(&tid) {
                            th.pr_url = Some(text.clone());
                        }
                        self.save_thread(&tid);
                        self.toast("Pull request opened.", true);
                    }
                    (DoneKind::Merge, true, Some(t)) => {
                        let tid = t.clone();
                        if let Some(th) = self.thread_mut(&tid) {
                            th.settled = true;
                            th.state = RunState::Completed;
                        }
                        self.save_thread(&tid);
                        self.toast(text, true);
                    }
                    (DoneKind::Restore, true, Some(t)) => {
                        let tid = t.clone();
                        self.checkpoints.remove(&tid);
                        let cps = self.store.load_checkpoints(&tid);
                        self.checkpoints.insert(tid.clone(), cps);
                        self.push_item(&tid, ItemKind::Notice { text: text.clone() });
                        self.toast(text, true);
                    }
                    (DoneKind::Remove, true, Some(t)) => {
                        let tid = t.clone();
                        if let Some(th) = self.thread_mut(&tid) {
                            th.cwd = None;
                            th.is_worktree = false;
                        }
                        self.save_thread(&tid);
                        self.toast(text, true);
                    }
                    _ => self.toast(text, ok),
                }
                if let Some(t) = thread {
                    self.refresh_git(&t);
                }
                if kind == DoneKind::Other {
                    self.scan_worktrees();
                }
            }
            Bg::Picked { purpose, paths } => match purpose {
                Pick::Project => {
                    if let Some(p) = paths.first() {
                        self.add_project(p);
                    }
                }
                Pick::Attach(None) => self.draft.attachments.extend(paths),
                Pick::Attach(Some(_)) => {
                    if let Some(id) = self.current.clone() {
                        self.ui.attachments.entry(id).or_default().extend(paths);
                    }
                }
            },
            Bg::Tests { thread, ok, summary } => {
                self.ui.busy.remove(&format!("tests:{thread}"));
                if let Some(t) = self.thread_mut(&thread) {
                    t.test_ok = ok;
                    t.test_summary = Some(summary);
                }
                self.save_thread(&thread);
            }
            Bg::Worktrees(list) => self.ui.cleanup_preview = Some(list),
        }
    }

    pub fn shutdown(&mut self) {
        self.save_ui();
        for (_, s) in self.sessions.drain() {
            drop(s);
        }
        for (_, mut t) in self.terminals.drain(..) {
            t.kill();
        }
        self.store.flush();
    }
}

// Results produced while handling another background result (multi-run worktree setup).
static DEFERRED: std::sync::Mutex<Vec<Bg>> = std::sync::Mutex::new(Vec::new());
pub fn defer_bg(b: Bg) {
    if let Ok(mut d) = DEFERRED.lock() {
        d.push(b);
    }
}
fn take_deferred() -> Option<Bg> {
    DEFERRED.lock().ok().and_then(|mut d| if d.is_empty() { None } else { Some(d.remove(0)) })
}

fn detect_scripts_sync_hint() -> Vec<(String, String)> {
    Vec::new()
}

pub fn fingerprint(f: &git::FileDiff) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    f.path.hash(&mut h);
    for hk in &f.hunks {
        hk.header.hash(&mut h);
        for l in &hk.lines {
            l.text.hash(&mut h);
        }
    }
    format!("{:016x}", h.finish())
}

pub fn same_path(a: &Path, b: &Path) -> bool {
    let n = |p: &Path| p.to_string_lossy().replace('\\', "/").trim_end_matches('/').to_ascii_lowercase();
    n(a) == n(b)
}

/// Is `p` inside directory `root`? (case-insensitive on Windows paths, separators normalised)
pub fn inside(root: &str, p: &str) -> bool {
    if root.is_empty() {
        return false;
    }
    let n = |s: &str| s.replace('\\', "/").trim_end_matches('/').to_string();
    let (r, q) = (n(root), n(p));
    let abs = q.starts_with('/') || q.chars().nth(1) == Some(':');
    if !abs {
        return !q.split('/').any(|c| c == "..");
    }
    let eq = if cfg!(windows) { q.to_ascii_lowercase().starts_with(&r.to_ascii_lowercase()) } else { q.starts_with(&r) };
    eq && (q.len() == r.len() || q.as_bytes()[r.len()] == b'/')
}

fn sanitize(s: &str) -> String {
    s.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '-' }).collect()
}

pub fn title_from(prompt: &str) -> String {
    let first = prompt.lines().find(|l| !l.trim().is_empty()).unwrap_or("New thread").trim();
    if first.chars().count() <= 48 {
        return first.to_string();
    }
    let cut: String = first.chars().take(48).collect();
    match cut.rfind(' ') {
        Some(i) if i > 20 => cut[..i].to_string(),
        _ => cut,
    }
}

use crate::model;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inside_worktree_checks() {
        assert!(inside("C:\\w\\proj", "C:\\w\\proj\\src\\a.rs"));
        assert!(inside("C:\\w\\proj", "c:/W/proj/src"));
        assert!(!inside("C:\\w\\proj", "C:\\w\\project2\\a"));
        assert!(!inside("C:\\w\\proj", "C:\\Users\\me\\.config\\x"));
        assert!(inside("C:\\w\\proj", "src/a.rs"));
        assert!(!inside("C:\\w\\proj", "../other/a.rs"));
    }

    #[test]
    fn titles() {
        assert_eq!(title_from("Fix BPF capture filter on Windows"), "Fix BPF capture filter on Windows");
        assert!(title_from("Capture filters like tcp port 443 work on Linux but match nothing on Windows. Find out why").len() <= 48);
    }
}
