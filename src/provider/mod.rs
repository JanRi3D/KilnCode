//! Shared provider interface: discovery, read-only health checks, capability registry,
//! session spawning and the normalized event stream the UI consumes.
//!
//! Kiln never talks to model APIs itself. Each adapter drives the user's installed agent
//! CLI over stdio using its documented structured protocol:
//! - Claude Code: `claude -p --input-format stream-json --output-format stream-json`
//!   with `--permission-prompt-tool stdio` (control_request / control_response).
//! - Codex: `codex app-server` (newline-delimited JSON-RPC 2.0 without the `jsonrpc` field).

pub mod claude;
pub mod codex;
pub mod process;

use crate::model::{PlanStep, ProviderKind, ToolKind, ToolStatus};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

#[derive(Clone, Debug, PartialEq)]
pub enum AuthState {
    Unknown,
    /// Signed in through the provider's own login (subscription or key, as the user configured it).
    SignedIn {
        method: String,
        account: Option<String>,
        plan: Option<String>,
    },
    SignedOut,
    Error(String),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ModelInfo {
    pub id: String,
    pub display: String,
    pub efforts: Vec<String>,
    pub default_effort: Option<String>,
    pub is_default: bool,
}

/// What each provider can actually do through the interface Kiln uses.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    pub resume: bool,
    pub interrupt: bool,
    /// Can a message be injected into a running turn?
    pub steer: bool,
    /// Provider-native "allow for this session" approvals.
    pub session_rules: bool,
    /// Can a decline carry feedback text to the agent?
    pub deny_feedback: bool,
    pub plan_updates: bool,
    pub token_usage: bool,
    pub rate_limits: bool,
    /// Reports an API-equivalent cost estimate (never a subscription charge).
    pub cost_estimate: bool,
    pub effort: bool,
    pub model_list: bool,
    /// Provider enforces an OS sandbox for commands.
    pub sandbox: bool,
}

pub fn capabilities(kind: ProviderKind) -> Capabilities {
    match kind {
        ProviderKind::Claude => Capabilities {
            resume: true,
            interrupt: true,
            steer: false,
            session_rules: true,
            deny_feedback: true,
            plan_updates: true,
            token_usage: true,
            rate_limits: true,
            cost_estimate: true,
            effort: true,
            model_list: false,
            sandbox: false,
        },
        ProviderKind::Codex => Capabilities {
            resume: true,
            interrupt: true,
            steer: true,
            session_rules: true,
            deny_feedback: false,
            plan_updates: true,
            token_usage: true,
            rate_limits: true,
            cost_estimate: false,
            effort: true,
            model_list: true,
            sandbox: true,
        },
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct RateLimit {
    pub label: String,
    pub used_percent: Option<f32>,
    pub resets_at: Option<i64>,
    pub status: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ProviderStatus {
    pub kind: ProviderKind,
    pub executable: Option<PathBuf>,
    pub version: Option<String>,
    pub auth: AuthState,
    pub models: Vec<ModelInfo>,
    pub rate_limits: Vec<RateLimit>,
    pub error: Option<String>,
    pub checked_at: i64,
}

impl ProviderStatus {
    pub fn unknown(kind: ProviderKind) -> Self {
        Self {
            kind,
            executable: None,
            version: None,
            auth: AuthState::Unknown,
            models: Vec::new(),
            rate_limits: Vec::new(),
            error: None,
            checked_at: 0,
        }
    }
    pub fn ready(&self) -> bool {
        self.executable.is_some() && matches!(self.auth, AuthState::SignedIn { .. })
    }
    pub fn summary(&self) -> String {
        if self.checked_at == 0 {
            return "Checking…".into();
        }
        if self.executable.is_none() {
            return "Not installed".into();
        }
        match &self.auth {
            AuthState::SignedIn { method, .. } => format!("Ready · signed in with {method}"),
            AuthState::SignedOut => "Installed · signed out".into(),
            AuthState::Error(e) => format!("Status unavailable · {e}"),
            AuthState::Unknown => "Installed · sign-in status unknown".into(),
        }
    }
    pub fn login_command(&self) -> &'static str {
        match self.kind {
            ProviderKind::Claude => "claude auth login",
            ProviderKind::Codex => "codex login",
        }
    }
    pub fn install_hint(&self) -> &'static str {
        match self.kind {
            ProviderKind::Claude => {
                "Install Claude Code from https://code.claude.com/docs (native installer or `winget install Anthropic.ClaudeCode`)."
            }
            ProviderKind::Codex => "Install the Codex CLI from https://developers.openai.com/codex (or `winget install OpenAI.Codex`).",
        }
    }
}

// ---------------------------------------------------------------------------------------
// Discovery

fn exe_names(base: &str) -> Vec<String> {
    if cfg!(windows) {
        vec![format!("{base}.exe"), format!("{base}.cmd"), format!("{base}.bat"), base.to_string()]
    } else {
        vec![base.to_string()]
    }
}

/// Finds a provider executable: explicit setting → PATH → well-known install locations.
/// Native binaries (`.exe`) win over script shims.
pub fn find_executable(kind: ProviderKind, configured: &str) -> Option<PathBuf> {
    let configured = configured.trim();
    if !configured.is_empty() {
        let p = PathBuf::from(configured);
        return p.is_file().then_some(p);
    }
    let base = kind.key();
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default();
    let home = crate::log::home_dir();
    if cfg!(windows) {
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            let local = PathBuf::from(local);
            dirs.push(local.join("Programs").join("OpenAI").join("Codex").join("bin"));
            dirs.push(local.join("Microsoft").join("WinGet").join("Links"));
            if let Ok(rd) = std::fs::read_dir(local.join("Microsoft").join("WinGet").join("Packages")) {
                dirs.extend(rd.flatten().map(|e| e.path()));
            }
        }
        if let Ok(appdata) = std::env::var("APPDATA") {
            dirs.push(PathBuf::from(appdata).join("npm"));
        }
    }
    dirs.push(home.join(".local").join("bin"));
    dirs.push(home.join(".claude").join("local"));
    dirs.push(PathBuf::from("/usr/local/bin"));
    dirs.push(PathBuf::from("/opt/homebrew/bin"));
    for name in exe_names(base) {
        for d in &dirs {
            let c = d.join(&name);
            if c.is_file() {
                return Some(c);
            }
        }
    }
    None
}

fn run_capture(exe: &Path, args: &[&str]) -> Result<(bool, String, String), String> {
    let mut cmd = process::command(exe);
    cmd.args(args).stdin(std::process::Stdio::null());
    let out = process::output_with_timeout(cmd, std::time::Duration::from_secs(20))?;
    Ok((
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).trim().to_string(),
        String::from_utf8_lossy(&out.stderr).trim().to_string(),
    ))
}

/// Read-only health check. Never starts a login flow, never changes provider settings,
/// never starts agent work.
pub fn check(kind: ProviderKind, configured_path: &str) -> ProviderStatus {
    let mut st = ProviderStatus::unknown(kind);
    st.checked_at = crate::model::now();
    let Some(exe) = find_executable(kind, configured_path) else {
        if !configured_path.trim().is_empty() {
            st.error = Some(format!("No executable at {}", configured_path.trim()));
        }
        return st;
    };
    st.executable = Some(exe.clone());
    match run_capture(&exe, &["--version"]) {
        Ok((true, out, _)) => st.version = Some(parse_version(&out)),
        Ok((false, out, err)) => st.error = Some(format!("--version failed: {}", if err.is_empty() { out } else { err })),
        Err(e) => st.error = Some(e),
    }
    match kind {
        ProviderKind::Claude => {
            st.auth = match run_capture(&exe, &["auth", "status", "--json"]) {
                Ok((_, out, err)) => claude::parse_auth_status(&out)
                    .unwrap_or_else(|| AuthState::Error(if err.is_empty() { "unrecognized output".into() } else { err })),
                Err(e) => AuthState::Error(e),
            };
            st.models = claude::model_aliases();
        }
        ProviderKind::Codex => {
            // Prefer the app-server's own account/model APIs; fall back to `codex login status`.
            match codex::probe(&exe) {
                Ok(p) => {
                    st.auth = p.auth;
                    st.models = p.models;
                    st.rate_limits = p.rate_limits;
                }
                Err(e) => {
                    st.auth = match run_capture(&exe, &["login", "status"]) {
                        Ok((ok, out, err)) => codex::parse_login_status(ok, &format!("{out}\n{err}")),
                        Err(e2) => AuthState::Error(format!("{e}; {e2}")),
                    };
                    st.error = Some(format!("app-server probe failed: {e}"));
                }
            }
        }
    }
    st
}

fn parse_version(s: &str) -> String {
    s.split_whitespace().find(|w| w.chars().next().is_some_and(|c| c.is_ascii_digit())).unwrap_or(s).to_string()
}

// ---------------------------------------------------------------------------------------
// Sessions

#[derive(Clone, Debug)]
pub struct SessionConfig {
    pub kind: ProviderKind,
    pub exe: PathBuf,
    pub cwd: PathBuf,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// Kiln permission preset: "Supervised", "Auto-accept edits", "Plan (read-only)", "Full access".
    pub permissions: String,
    /// Provider-native session id to resume, if any.
    pub resume: Option<String>,
    pub thread_id: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Decision {
    Allow,
    AllowForSession,
    Deny {
        message: Option<String>,
    },
    /// Decline and stop the turn.
    Abort,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApprovalKind {
    Command,
    FileChange,
    Network,
    Permissions,
    Tool,
}

#[derive(Clone, Debug)]
pub struct ApprovalRequest {
    /// Adapter-level key used to answer this exact provider request.
    pub request_id: String,
    pub kind: ApprovalKind,
    pub title: String,
    pub tool_name: Option<String>,
    pub command: Option<String>,
    pub cwd: Option<String>,
    pub paths: Vec<String>,
    pub reason: Option<String>,
    pub preview: Option<String>,
    /// Provider classified the command as read/search/list only.
    pub read_only: bool,
    pub network: bool,
    /// Text for the provider-native session-scoped allow, when the provider offers one.
    pub session_option: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TurnOutcome {
    Completed,
    Interrupted,
    Failed,
}

#[derive(Clone, Debug)]
pub enum Ev {
    Started { native_id: String, model: Option<String>, permission_mode: Option<String>, auth_source: Option<String> },
    TurnStarted,
    TextDelta { id: String, text: String },
    Text { id: String, text: String },
    ReasoningDelta { id: String, text: String },
    ToolStarted { id: String, kind: ToolKind, label: String, detail: Option<String>, paths: Vec<String> },
    ToolOutput { id: String, text: String },
    ToolDone { id: String, status: ToolStatus, output: Option<String> },
    Approval(ApprovalRequest),
    ApprovalGone { request_id: String },
    Plan(Vec<PlanStep>),
    Usage { context_tokens: Option<u64>, context_window: Option<u64>, input: u64, output: u64, cached: u64, cost_usd: Option<f64> },
    RateLimit(RateLimit),
    Models(Vec<ModelInfo>),
    FilesTouched,
    TurnDone { outcome: TurnOutcome, error: Option<String> },
    Error { message: String, will_retry: bool },
    Log(String),
    Exited { code: Option<i32>, stderr_tail: String },
}

#[derive(Clone, Debug)]
pub struct ProviderEvent {
    pub thread_id: String,
    pub ev: Ev,
}

#[derive(Debug)]
pub enum Cmd {
    Send { text: String, attachments: Vec<PathBuf>, steer: bool },
    Interrupt,
    Approve { request_id: String, decision: Decision },
    Shutdown,
}

/// Owned handle to a running provider session. Dropping it shuts the process down.
pub struct SessionHandle {
    tx: mpsc::Sender<Cmd>,
}

impl SessionHandle {
    pub fn send(&self, cmd: Cmd) -> bool {
        self.tx.send(cmd).is_ok()
    }
}

impl Drop for SessionHandle {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Shutdown);
    }
}

/// Emits events to the UI and wakes it up.
#[derive(Clone)]
pub struct Sink {
    pub thread_id: String,
    pub tx: mpsc::Sender<ProviderEvent>,
    pub ctx: Option<egui::Context>,
}

impl Sink {
    pub fn emit(&self, ev: Ev) {
        let _ = self.tx.send(ProviderEvent { thread_id: self.thread_id.clone(), ev });
        if let Some(c) = &self.ctx {
            c.request_repaint();
        }
    }
}

pub fn spawn(cfg: SessionConfig, tx: mpsc::Sender<ProviderEvent>, ctx: egui::Context) -> Result<SessionHandle, String> {
    let sink = Sink { thread_id: cfg.thread_id.clone(), tx, ctx: Some(ctx) };
    let (ctx_tx, ctx_rx) = mpsc::channel();
    match cfg.kind {
        ProviderKind::Claude => claude::start(cfg, sink, ctx_rx)?,
        ProviderKind::Codex => codex::start(cfg, sink, ctx_rx)?,
    }
    Ok(SessionHandle { tx: ctx_tx })
}

/// Short human label for a tool invocation, e.g. `Read src/main.rs`.
pub fn short(s: &str, n: usize) -> String {
    let s = s.trim();
    let first = s.lines().next().unwrap_or("");
    if first.chars().count() <= n && !s.contains('\n') {
        first.to_string()
    } else {
        let mut out: String = first.chars().take(n).collect();
        out.push('…');
        out
    }
}

pub fn str_field<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k).and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse() {
        assert_eq!(parse_version("2.1.240 (Claude Code)"), "2.1.240");
        assert_eq!(parse_version("codex-cli 0.166.0"), "0.166.0");
    }

    #[test]
    fn capability_registry_is_honest() {
        let c = capabilities(ProviderKind::Claude);
        assert!(c.resume && c.interrupt && c.cost_estimate && !c.model_list && !c.sandbox);
        let x = capabilities(ProviderKind::Codex);
        assert!(x.steer && x.model_list && x.sandbox && !x.cost_estimate && !x.deny_feedback);
    }

    #[test]
    fn missing_configured_path_is_reported_not_guessed() {
        let st = check(ProviderKind::Claude, "Z:\\definitely\\not\\here\\claude.exe");
        assert!(st.executable.is_none());
        assert!(st.error.as_deref().unwrap().contains("No executable"));
        assert_eq!(st.summary(), "Not installed");
    }
}

#[cfg(test)]
mod live {
    #[test]
    #[ignore]
    fn live_codex_probe() {
        let t = std::time::Instant::now();
        let exe = super::find_executable(crate::model::ProviderKind::Codex, "").expect("codex");
        println!("exe {}", exe.display());
        let r = super::codex::probe(&exe);
        println!(
            "probe after {:?}: {:?}",
            t.elapsed(),
            r.as_ref().map(|p| (&p.auth, p.models.len(), p.rate_limits.len())).map_err(|e| e.clone())
        );
        assert!(r.is_ok());
    }
}
