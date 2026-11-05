//! Codex adapter: drives `codex app-server` over stdio.
//!
//! Protocol verified against the JSON Schema emitted by the installed binary
//! (`codex app-server generate-json-schema`, codex-cli 0.166.0):
//! newline-delimited JSON-RPC 2.0 records without a `jsonrpc` field; `initialize` →
//! `initialized`; `thread/start|resume`; `turn/start|steer|interrupt`; server→client approval
//! requests answered with `{decision}`. Codex manages its own login (ChatGPT or API key) and
//! enforces its sandbox; Kiln only chooses the per-turn approval policy and sandbox preset.

use super::process::{self, Managed};
use super::*;
use crate::model::{PlanStep, StepStatus, ToolKind, ToolStatus};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

type Reply = mpsc::Sender<Result<Value, String>>;

pub struct Rpc {
    m: Arc<Managed>,
    next: AtomicI64,
    waiting: Mutex<HashMap<i64, Reply>>,
}

impl Rpc {
    fn new(m: Arc<Managed>) -> Arc<Self> {
        Arc::new(Self { m, next: AtomicI64::new(1), waiting: Mutex::default() })
    }
    pub fn call(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel();
        self.waiting.lock().map_err(|_| "poisoned")?.insert(id, tx);
        self.m.write_line(&json!({ "id": id, "method": method, "params": params }).to_string())?;
        match rx.recv_timeout(timeout) {
            Ok(r) => r,
            Err(_) => {
                self.waiting.lock().ok().map(|mut w| w.remove(&id));
                Err(format!("{method} timed out"))
            }
        }
    }
    pub fn notify(&self, method: &str, params: Option<Value>) -> Result<(), String> {
        let mut v = json!({ "method": method });
        if let Some(p) = params {
            v["params"] = p;
        }
        self.m.write_line(&v.to_string())
    }
    pub fn respond(&self, id: &Value, result: Value) -> Result<(), String> {
        self.m.write_line(&json!({ "id": id, "result": result }).to_string())
    }
    pub fn respond_error(&self, id: &Value, code: i64, message: &str) -> Result<(), String> {
        self.m.write_line(&json!({ "id": id, "error": { "code": code, "message": message } }).to_string())
    }
    /// Routes a response record to the waiting caller. Returns false if it wasn't a response.
    fn on_response(&self, v: &Value) -> bool {
        if v.get("method").is_some() || (v.get("result").is_none() && v.get("error").is_none()) {
            return false;
        }
        let Some(id) = v["id"].as_i64() else { return true };
        if let Some(tx) = self.waiting.lock().ok().and_then(|mut w| w.remove(&id)) {
            let r = match v.get("error") {
                Some(e) => Err(e["message"].as_str().unwrap_or("error").to_string()),
                None => Ok(v["result"].clone()),
            };
            let _ = tx.send(r);
        }
        true
    }
    fn fail_all(&self, why: &str) {
        if let Ok(mut w) = self.waiting.lock() {
            for (_, tx) in w.drain() {
                let _ = tx.send(Err(why.to_string()));
            }
        }
    }
}

/// Per-turn approval policy and sandbox for a Kiln permission preset.
pub fn policy(preset: &str) -> (&'static str, Value) {
    match preset {
        "Auto-accept edits" => ("on-request", json!({ "type": "workspaceWrite" })),
        "Plan (read-only)" => ("never", json!({ "type": "readOnly" })),
        "Full access" => ("never", json!({ "type": "dangerFullAccess" })),
        _ => ("untrusted", json!({ "type": "readOnly" })),
    }
}

#[derive(Clone, Debug)]
pub enum PendingKind {
    Command,
    File,
    Permissions(Value),
    Legacy,
}

pub type Pending = Arc<Mutex<HashMap<String, (Value, PendingKind)>>>;

/// What to answer immediately for server requests Kiln doesn't support.
#[derive(Debug, PartialEq)]
pub struct AutoError {
    pub id: Value,
    pub message: String,
}

pub struct Parser {
    pub cwd: String,
    pub pending: Pending,
    pub turn: Arc<Mutex<Option<String>>>,
    files: HashMap<String, (Vec<String>, String)>,
    streamed: HashSet<String>,
}

impl Parser {
    pub fn new(cwd: &str) -> Self {
        Self { cwd: cwd.to_string(), pending: Arc::default(), turn: Arc::default(), files: HashMap::new(), streamed: HashSet::new() }
    }

    fn rel(&self, p: &str) -> String {
        super::claude::rel_path(&self.cwd, p)
    }

    /// Handles a notification or server request.
    pub fn feed(&mut self, v: &Value) -> (Vec<Ev>, Option<AutoError>) {
        let method = str_field(v, "method").unwrap_or("");
        let p = &v["params"];
        let mut out = Vec::new();
        if let Some(id) = v.get("id").filter(|_| !method.is_empty()) {
            return self.server_request(id.clone(), method, p);
        }
        match method {
            "turn/started" => {
                if let Some(id) = p["turn"]["id"].as_str() {
                    *self.turn.lock().unwrap_or_else(|e| e.into_inner()) = Some(id.to_string());
                }
                out.push(Ev::TurnStarted);
            }
            "turn/completed" => {
                let t = &p["turn"];
                *self.turn.lock().unwrap_or_else(|e| e.into_inner()) = None;
                let outcome = match str_field(t, "status") {
                    Some("interrupted") => TurnOutcome::Interrupted,
                    Some("failed") => TurnOutcome::Failed,
                    _ => TurnOutcome::Completed,
                };
                let error = t["error"]["message"].as_str().map(str::to_string);
                out.push(Ev::TurnDone { outcome, error });
            }
            "item/agentMessage/delta" => {
                let id = str_field(p, "itemId").unwrap_or_default().to_string();
                out.push(Ev::TextDelta { id, text: str_field(p, "delta").unwrap_or_default().to_string() });
            }
            "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => {
                let id = str_field(p, "itemId").unwrap_or_default().to_string();
                self.streamed.insert(id.clone());
                out.push(Ev::ReasoningDelta { id, text: str_field(p, "delta").unwrap_or_default().to_string() });
            }
            "item/commandExecution/outputDelta" => {
                out.push(Ev::ToolOutput {
                    id: str_field(p, "itemId").unwrap_or_default().to_string(),
                    text: str_field(p, "delta").unwrap_or_default().to_string(),
                });
            }
            "item/started" => self.item(&p["item"], false, &mut out),
            "item/completed" => self.item(&p["item"], true, &mut out),
            "turn/plan/updated" => {
                let steps = p["plan"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .map(|s| PlanStep {
                                step: str_field(s, "step").unwrap_or_default().to_string(),
                                status: match str_field(s, "status") {
                                    Some("completed") => StepStatus::Completed,
                                    Some("inProgress") => StepStatus::InProgress,
                                    _ => StepStatus::Pending,
                                },
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                out.push(Ev::Plan(steps));
            }
            "turn/diff/updated" => out.push(Ev::FilesTouched),
            "thread/tokenUsage/updated" => {
                let u = &p["tokenUsage"];
                out.push(Ev::Usage {
                    context_tokens: u["last"]["totalTokens"].as_u64(),
                    context_window: u["modelContextWindow"].as_u64(),
                    input: u["total"]["inputTokens"].as_u64().unwrap_or(0),
                    output: u["total"]["outputTokens"].as_u64().unwrap_or(0),
                    cached: u["total"]["cachedInputTokens"].as_u64().unwrap_or(0),
                    cost_usd: None,
                });
            }
            "account/rateLimits/updated" => out.extend(rate_limits(&p["rateLimits"]).into_iter().map(Ev::RateLimit)),
            "error" => out.push(Ev::Error {
                message: p["error"]["message"].as_str().unwrap_or("Codex reported an error").to_string(),
                will_retry: p["willRetry"].as_bool().unwrap_or(false),
            }),
            "serverRequest/resolved" => {
                let id = p["requestId"].to_string();
                self.pending.lock().ok().map(|mut x| x.remove(&id));
                out.push(Ev::ApprovalGone { request_id: id });
            }
            "warning" | "configWarning" | "deprecationNotice" | "guardianWarning" | "windows/worldWritableWarning" => {
                out.push(Ev::Log(format!("codex {method}: {}", short(&p.to_string(), 300))));
            }
            "thread/compacted" => out.push(Ev::Log("Codex compacted the conversation context.".into())),
            "model/rerouted" => out.push(Ev::Log(format!("Codex rerouted the model: {}", short(&p.to_string(), 200)))),
            _ => {}
        }
        (out, None)
    }

    fn item(&mut self, it: &Value, done: bool, out: &mut Vec<Ev>) {
        let id = str_field(it, "id").unwrap_or_default().to_string();
        match str_field(it, "type").unwrap_or("") {
            "agentMessage" if done => out.push(Ev::Text { id, text: str_field(it, "text").unwrap_or_default().to_string() }),
            "reasoning" if done && !self.streamed.contains(&id) => {
                let s: Vec<&str> = it["summary"].as_array().map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
                if !s.is_empty() {
                    out.push(Ev::ReasoningDelta { id, text: s.join("\n\n") });
                }
            }
            "commandExecution" => {
                if done {
                    let status = match str_field(it, "status") {
                        Some("declined") => ToolStatus::Declined,
                        Some("failed") => ToolStatus::Failed,
                        _ if it["exitCode"].as_i64().is_some_and(|c| c != 0) => ToolStatus::Failed,
                        _ => ToolStatus::Ok,
                    };
                    out.push(Ev::FilesTouched);
                    out.push(Ev::ToolDone {
                        id,
                        status,
                        output: str_field(it, "aggregatedOutput").map(|s| super::claude::truncate(s, 8000)),
                    });
                } else {
                    let cmd = str_field(it, "command").unwrap_or_default();
                    let (kind, label) = self.describe_command(cmd, &it["commandActions"]);
                    out.push(Ev::ToolStarted { id, kind, label, detail: Some(cmd.to_string()), paths: Vec::new() });
                }
            }
            "fileChange" => {
                let changes = it["changes"].as_array().cloned().unwrap_or_default();
                let paths: Vec<String> = changes.iter().filter_map(|c| str_field(c, "path")).map(|p| self.rel(p)).collect();
                let preview = changes.iter().filter_map(|c| str_field(c, "diff")).collect::<Vec<_>>().join("\n");
                self.files.insert(id.clone(), (paths.clone(), super::claude::truncate(&preview, 4000)));
                if done {
                    let status = match str_field(it, "status") {
                        Some("declined") => ToolStatus::Declined,
                        Some("failed") => ToolStatus::Failed,
                        _ => ToolStatus::Ok,
                    };
                    out.push(Ev::FilesTouched);
                    out.push(Ev::ToolDone { id, status, output: None });
                } else {
                    let label = match paths.as_slice() {
                        [one] => format!("Edited {one}"),
                        many => format!("Edited {} files", many.len()),
                    };
                    out.push(Ev::ToolStarted { id, kind: ToolKind::Edit, label, detail: None, paths });
                }
            }
            "mcpToolCall" => {
                if done {
                    let ok = it["error"].is_null() && str_field(it, "status") != Some("failed");
                    out.push(Ev::ToolDone {
                        id,
                        status: if ok { ToolStatus::Ok } else { ToolStatus::Failed },
                        output: Some(short(&it["result"].to_string(), 2000)),
                    });
                } else {
                    out.push(Ev::ToolStarted {
                        id,
                        kind: ToolKind::Mcp,
                        label: format!("Called {} · {}", str_field(it, "server").unwrap_or(""), str_field(it, "tool").unwrap_or("")),
                        detail: Some(short(&it["arguments"].to_string(), 400)),
                        paths: Vec::new(),
                    });
                }
            }
            "webSearch" => {
                if done {
                    out.push(Ev::ToolDone { id, status: ToolStatus::Ok, output: None });
                } else {
                    out.push(Ev::ToolStarted {
                        id,
                        kind: ToolKind::Web,
                        label: format!("Searched the web for \"{}\"", str_field(it, "query").unwrap_or("")),
                        detail: None,
                        paths: Vec::new(),
                    });
                }
            }
            "contextCompaction" if done => out.push(Ev::Log("Codex compacted the context.".into())),
            "userMessage" | "agentMessage" | "reasoning" | "plan" | "hookPrompt" | "contextCompaction" => {}
            other => {
                if done {
                    out.push(Ev::ToolDone { id, status: ToolStatus::Ok, output: None });
                } else {
                    out.push(Ev::ToolStarted {
                        id,
                        kind: ToolKind::Other,
                        label: format!("Used {other}"),
                        detail: None,
                        paths: Vec::new(),
                    });
                }
            }
        }
    }

    fn describe_command(&self, cmd: &str, actions: &Value) -> (ToolKind, String) {
        let acts = actions.as_array().cloned().unwrap_or_default();
        if acts.len() == 1 {
            let a = &acts[0];
            let path = str_field(a, "path").map(|p| self.rel(p));
            match str_field(a, "type") {
                Some("read") => {
                    return (
                        ToolKind::Read,
                        format!("Read {}", path.or_else(|| str_field(a, "name").map(str::to_string)).unwrap_or_default()),
                    );
                }
                Some("search") => {
                    let q = str_field(a, "query").map(|q| format!("\"{q}\"")).unwrap_or_else(|| "files".into());
                    return (ToolKind::Search, format!("Searched for {q}{}", path.map(|p| format!(" in {p}")).unwrap_or_default()));
                }
                Some("listFiles") => return (ToolKind::List, format!("Listed {}", path.unwrap_or_else(|| ".".into()))),
                _ => {}
            }
        }
        (ToolKind::Command, format!("Ran {}", short(&strip_shell_wrapper(cmd), 90)))
    }

    fn server_request(&mut self, id: Value, method: &str, p: &Value) -> (Vec<Ev>, Option<AutoError>) {
        let key = id.to_string();
        let cwd = str_field(p, "cwd").map(str::to_string).or_else(|| Some(self.cwd.clone()));
        let mut a = ApprovalRequest {
            request_id: key.clone(),
            kind: ApprovalKind::Command,
            title: "Run this command?".into(),
            tool_name: None,
            command: None,
            cwd,
            paths: Vec::new(),
            reason: str_field(p, "reason").map(str::to_string),
            preview: None,
            read_only: false,
            network: false,
            session_option: None,
        };
        let pk = match method {
            "item/commandExecution/requestApproval" => {
                a.command = str_field(p, "command").map(strip_shell_wrapper);
                let acts = p["commandActions"].as_array().cloned().unwrap_or_default();
                a.read_only =
                    !acts.is_empty() && acts.iter().all(|x| matches!(str_field(x, "type"), Some("read" | "search" | "listFiles")));
                if !p["networkApprovalContext"].is_null() {
                    a.kind = ApprovalKind::Network;
                    a.network = true;
                    a.title = "Allow network access?".into();
                }
                a.session_option = Some("Allow for this session".into());
                PendingKind::Command
            }
            "item/fileChange/requestApproval" => {
                a.kind = ApprovalKind::FileChange;
                a.title = "Apply these file changes?".into();
                if let Some((paths, preview)) = str_field(p, "itemId").and_then(|i| self.files.get(i)) {
                    a.paths = paths.clone();
                    a.command = Some(paths.join(", "));
                    a.preview = Some(preview.clone());
                }
                if let Some(root) = str_field(p, "grantRoot") {
                    a.paths.push(root.to_string());
                }
                a.session_option = Some("Allow these files for this session".into());
                PendingKind::File
            }
            "item/permissions/requestApproval" => {
                a.kind = ApprovalKind::Permissions;
                a.title = "Grant additional permissions?".into();
                a.network = !p["permissions"]["network"].is_null();
                a.command = Some(short(&p["permissions"].to_string(), 300));
                a.session_option = Some("Grant for this session".into());
                PendingKind::Permissions(p["permissions"].clone())
            }
            "execCommandApproval" => {
                a.command = p["command"].as_array().map(|c| c.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" "));
                PendingKind::Legacy
            }
            "applyPatchApproval" => {
                a.kind = ApprovalKind::FileChange;
                a.title = "Apply these file changes?".into();
                a.paths = p["fileChanges"].as_object().map(|m| m.keys().map(|k| self.rel(k)).collect()).unwrap_or_default();
                a.command = Some(a.paths.join(", "));
                PendingKind::Legacy
            }
            other => {
                return (
                    vec![Ev::Log(format!("Codex asked for {other}, which Kiln does not support; declined."))],
                    Some(AutoError { id, message: format!("Kiln Code does not support {other}") }),
                );
            }
        };
        if let Ok(mut x) = self.pending.lock() {
            x.insert(key, (id, pk));
        }
        (vec![Ev::Approval(a)], None)
    }
}

/// `"C:\\WINDOWS\\...\\powershell.exe" -Command 'cargo test'` → `cargo test` for display/rules.
pub fn strip_shell_wrapper(cmd: &str) -> String {
    let c = cmd.trim();
    for marker in ["-Command ", "-c ", "/c "] {
        if let Some(i) = c.find(marker) {
            let head = c[..i].to_ascii_lowercase();
            if head.contains("powershell")
                || head.contains("pwsh")
                || head.contains("bash")
                || head.ends_with("sh ")
                || head.contains("cmd")
            {
                let rest = c[i + marker.len()..].trim();
                let unq = rest
                    .strip_prefix('\'')
                    .and_then(|r| r.strip_suffix('\''))
                    .or_else(|| rest.strip_prefix('"').and_then(|r| r.strip_suffix('"')))
                    .unwrap_or(rest);
                return unq.to_string();
            }
        }
    }
    c.to_string()
}

pub fn decision_result(kind: &PendingKind, d: &Decision) -> Value {
    match kind {
        PendingKind::Command | PendingKind::File => json!({ "decision": match d {
            Decision::Allow => "accept",
            Decision::AllowForSession => "acceptForSession",
            Decision::Deny { .. } => "decline",
            Decision::Abort => "cancel",
        }}),
        PendingKind::Permissions(req) => match d {
            Decision::Allow => json!({ "permissions": req, "scope": "turn" }),
            Decision::AllowForSession => json!({ "permissions": req, "scope": "session" }),
            _ => json!({ "permissions": {}, "scope": "turn" }),
        },
        PendingKind::Legacy => json!({ "decision": match d {
            Decision::Allow => "approved",
            Decision::AllowForSession => "approved_for_session",
            Decision::Deny { .. } => "denied",
            Decision::Abort => "abort",
        }}),
    }
}

pub fn rate_limits(snap: &Value) -> Vec<RateLimit> {
    let mut out = Vec::new();
    for key in ["primary", "secondary"] {
        let w = &snap[key];
        if w.is_null() {
            continue;
        }
        let mins = w["windowDurationMins"].as_i64();
        let label = match mins {
            Some(m) if m >= 1440 && m % 1440 == 0 => format!("{}-day window", m / 1440),
            Some(m) if m >= 60 => format!("{}-hour window", m / 60),
            Some(m) => format!("{m}-minute window"),
            None => key.to_string(),
        };
        out.push(RateLimit {
            label,
            used_percent: w["usedPercent"].as_f64().map(|u| u as f32),
            resets_at: w["resetsAt"].as_i64(),
            status: None,
        });
    }
    out
}

pub fn parse_login_status(ok: bool, text: &str) -> AuthState {
    let t = text.to_ascii_lowercase();
    if t.contains("not logged in") {
        AuthState::SignedOut
    } else if t.contains("logged in using chatgpt") {
        AuthState::SignedIn { method: "ChatGPT".into(), account: None, plan: None }
    } else if t.contains("api key") && t.contains("logged in") {
        AuthState::SignedIn { method: "an API key".into(), account: None, plan: None }
    } else if ok && t.contains("logged in") {
        AuthState::SignedIn { method: "Codex login".into(), account: None, plan: None }
    } else {
        AuthState::Error(text.trim().lines().next().unwrap_or("unknown").to_string())
    }
}

pub fn parse_account(v: &Value) -> AuthState {
    match v["account"]["type"].as_str() {
        Some("chatgpt") => AuthState::SignedIn {
            method: "ChatGPT".into(),
            account: str_field(&v["account"], "email").map(str::to_string),
            plan: str_field(&v["account"], "planType").map(str::to_string),
        },
        Some("apiKey") => AuthState::SignedIn { method: "an API key".into(), account: None, plan: None },
        Some(other) => AuthState::SignedIn { method: other.to_string(), account: None, plan: None },
        None if v["requiresOpenaiAuth"].as_bool() == Some(false) => {
            AuthState::SignedIn { method: "a custom model provider".into(), account: None, plan: None }
        }
        None => AuthState::SignedOut,
    }
}

pub fn parse_models(v: &Value) -> Vec<ModelInfo> {
    v["data"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|m| m["hidden"].as_bool() != Some(true))
                .map(|m| ModelInfo {
                    id: str_field(m, "model").or_else(|| str_field(m, "id")).unwrap_or_default().to_string(),
                    display: str_field(m, "displayName").unwrap_or_default().to_string(),
                    efforts: m["supportedReasoningEfforts"]
                        .as_array()
                        .map(|e| e.iter().filter_map(|x| str_field(x, "reasoningEffort")).map(str::to_string).collect())
                        .unwrap_or_default(),
                    default_effort: str_field(m, "defaultReasoningEffort").map(str::to_string),
                    is_default: m["isDefault"].as_bool().unwrap_or(false),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn client_info() -> Value {
    json!({ "clientInfo": { "name": "kiln_code", "title": "Kiln Code", "version": env!("CARGO_PKG_VERSION") }, "capabilities": { "experimentalApi": true } })
}

fn spawn_server(
    exe: &std::path::Path,
    cwd: Option<&std::path::Path>,
) -> Result<(Arc<Managed>, std::process::ChildStdout, std::process::ChildStderr), String> {
    let mut cmd = process::command(exe);
    cmd.arg("app-server");
    if let Some(c) = cwd {
        cmd.current_dir(c);
    }
    Managed::spawn(cmd)
}

pub struct Probe {
    pub auth: AuthState,
    pub models: Vec<ModelInfo>,
    pub rate_limits: Vec<RateLimit>,
}

/// Read-only probe: initialize, account/read (no token refresh), model/list, rate limits.
pub fn probe(exe: &std::path::Path) -> Result<Probe, String> {
    let (m, stdout, stderr) = spawn_server(exe, None)?;
    let rpc = Rpc::new(m.clone());
    let r2 = rpc.clone();
    std::thread::spawn(move || {
        process::read_lines(stdout, |line, _| {
            if let Ok(v) = serde_json::from_str::<Value>(line) {
                r2.on_response(&v);
            }
        });
        r2.fail_all("codex app-server exited");
    });
    std::thread::spawn(move || process::read_lines(stderr, |_, _| {}));
    let t = Duration::from_secs(20);
    let result = (|| {
        rpc.call("initialize", client_info(), t)?;
        rpc.notify("initialized", None)?;
        let acct = rpc.call("account/read", json!({ "refreshToken": false }), t)?;
        let models = rpc.call("model/list", json!({}), t).map(|v| parse_models(&v)).unwrap_or_default();
        let limits = rpc.call("account/rateLimits/read", Value::Null, t).map(|v| rate_limits(&v["rateLimits"])).unwrap_or_default();
        Ok(Probe { auth: parse_account(&acct), models, rate_limits: limits })
    })();
    m.shutdown(Duration::from_secs(2));
    result
}

fn user_input(text: &str, attachments: &[std::path::PathBuf]) -> Value {
    let mut items = Vec::new();
    let mut files = Vec::new();
    for a in attachments {
        let ext = a.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        if ["png", "jpg", "jpeg", "gif", "webp"].contains(&ext.as_str()) {
            items.push(json!({ "type": "localImage", "path": a.display().to_string() }));
        } else {
            files.push(a.display().to_string());
        }
    }
    let mut t = text.to_string();
    if !files.is_empty() {
        t.push_str("\n\nAttached files:\n");
        for f in files {
            t.push_str(&format!("- {f}\n"));
        }
    }
    items.insert(0, json!({ "type": "text", "text": t, "text_elements": [] }));
    Value::Array(items)
}

pub fn start(cfg: SessionConfig, sink: Sink, rx: mpsc::Receiver<Cmd>) -> Result<(), String> {
    crate::log::info(&format!("starting codex app-server in {} (resume: {})", cfg.cwd.display(), cfg.resume.is_some()));
    let (m, stdout, stderr) = spawn_server(&cfg.exe, Some(&cfg.cwd))?;
    let tail = process::stderr_collector(stderr, sink.clone());
    let rpc = Rpc::new(m.clone());
    let mut parser = Parser::new(&cfg.cwd.to_string_lossy());
    let pending = parser.pending.clone();
    let turn = parser.turn.clone();

    let (r_rpc, r_sink, r_m) = (rpc.clone(), sink.clone(), m.clone());
    std::thread::Builder::new()
        .name("codex-reader".into())
        .spawn(move || {
            process::read_lines(stdout, |line, complete| {
                let v: Value = match serde_json::from_str(line) {
                    Ok(v) => v,
                    Err(e) => {
                        r_sink.emit(if complete {
                            Ev::Log(format!("unparseable record from codex: {e}"))
                        } else {
                            Ev::Error { message: "Codex's output ended in the middle of a message.".into(), will_retry: false }
                        });
                        return;
                    }
                };
                if r_rpc.on_response(&v) {
                    return;
                }
                let (evs, auto) = parser.feed(&v);
                if let Some(a) = auto {
                    let _ = r_rpc.respond_error(&a.id, -32601, &a.message);
                }
                for e in evs {
                    r_sink.emit(e);
                }
            });
            r_rpc.fail_all("codex app-server exited");
            let code = (0..100).find_map(|_| {
                let c = r_m.try_exit_code();
                if c.is_none() {
                    std::thread::sleep(Duration::from_millis(20));
                }
                c
            });
            r_sink.emit(Ev::Exited { code: code.flatten(), stderr_tail: tail.lock().map(|t| t.clone()).unwrap_or_default() });
        })
        .map_err(|e| e.to_string())?;

    std::thread::Builder::new()
        .name("codex-driver".into())
        .spawn(move || {
            let t = Duration::from_secs(60);
            let cwd = cfg.cwd.to_string_lossy().to_string();
            let model = cfg.model.clone().filter(|m| !m.is_empty() && m != "default");
            let setup = (|| -> Result<String, String> {
                rpc.call("initialize", client_info(), t)?;
                rpc.notify("initialized", None)?;
                let mut params = json!({ "cwd": cwd });
                if let Some(m) = &model {
                    params["model"] = json!(m);
                }
                let res = match &cfg.resume {
                    Some(id) => {
                        let mut rp = params.clone();
                        rp["threadId"] = json!(id);
                        rp["excludeTurns"] = json!(true);
                        match rpc.call("thread/resume", rp, t) {
                            Ok(r) => r,
                            Err(e) => {
                                sink.emit(Ev::Error { message: format!("Could not resume the Codex thread ({e}). Started a new Codex thread; it does not have the earlier context."), will_retry: false });
                                rpc.call("thread/start", params, t)?
                            }
                        }
                    }
                    None => rpc.call("thread/start", params, t)?,
                };
                let id = res["thread"]["id"].as_str().ok_or("thread/start returned no thread id")?.to_string();
                sink.emit(Ev::Started { native_id: id.clone(), model: str_field(&res, "model").map(str::to_string), permission_mode: None, auth_source: None });
                if let Ok(models) = rpc.call("model/list", json!({}), t) {
                    sink.emit(Ev::Models(parse_models(&models)));
                }
                if let Ok(rl) = rpc.call("account/rateLimits/read", Value::Null, t) {
                    for r in rate_limits(&rl["rateLimits"]) {
                        sink.emit(Ev::RateLimit(r));
                    }
                }
                Ok(id)
            })();
            let thread_id = match setup {
                Ok(id) => id,
                Err(e) => {
                    sink.emit(Ev::Error { message: format!("Codex app-server failed to start a thread: {e}"), will_retry: false });
                    m.shutdown(Duration::from_secs(2));
                    return;
                }
            };
            let (approval, sandbox) = policy(&cfg.permissions);
            for cmd in rx {
                let r: Result<(), String> = match cmd {
                    Cmd::Send { text, attachments, steer } => {
                        let input = user_input(&text, &attachments);
                        let active = turn.lock().ok().and_then(|g| g.clone());
                        match (steer, active) {
                            (true, Some(turn_id)) => rpc.call("turn/steer", json!({ "threadId": thread_id, "expectedTurnId": turn_id, "input": input }), t).map(|_| ()),
                            _ => {
                                let mut params = json!({ "threadId": thread_id, "input": input, "cwd": cwd, "approvalPolicy": approval, "sandboxPolicy": sandbox, "summary": "auto" });
                                if let Some(m) = &model {
                                    params["model"] = json!(m);
                                }
                                if let Some(e) = cfg.effort.as_deref().filter(|e| !e.is_empty()) {
                                    params["effort"] = json!(e);
                                }
                                rpc.call("turn/start", params, t).map(|r| {
                                    if let Some(id) = r["turn"]["id"].as_str() {
                                        *turn.lock().unwrap_or_else(|e| e.into_inner()) = Some(id.to_string());
                                    }
                                })
                            }
                        }
                    }
                    Cmd::Interrupt => match turn.lock().ok().and_then(|g| g.clone()) {
                        Some(turn_id) => rpc.call("turn/interrupt", json!({ "threadId": thread_id, "turnId": turn_id }), t).map(|_| ()),
                        None => Ok(()),
                    },
                    Cmd::Approve { request_id, decision } => match pending.lock().ok().and_then(|mut p| p.remove(&request_id)) {
                        Some((raw, kind)) => rpc.respond(&raw, decision_result(&kind, &decision)),
                        None => Err("That request is no longer pending.".into()),
                    },
                    Cmd::Shutdown => break,
                };
                if let Err(e) = r {
                    sink.emit(Ev::Error { message: e, will_retry: false });
                }
            }
            let _ = rpc.call("thread/unsubscribe", json!({ "threadId": thread_id }), Duration::from_secs(2));
            m.shutdown(Duration::from_secs(3));
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Value {
        serde_json::from_str(s).unwrap()
    }

    #[test]
    fn command_approval_round_trip() {
        let mut p = Parser::new("C:\\w");
        let (evs, auto) = p.feed(&v(r#"{"id":0,"method":"item/commandExecution/requestApproval","params":{"itemId":"call_1","threadId":"t","turnId":"u","startedAtMs":1,"command":"\"C:\\\\WINDOWS\\\\System32\\\\WindowsPowerShell\\\\v1.0\\\\powershell.exe\" -Command 'cargo test'","cwd":"C:\\w","commandActions":[{"type":"unknown","command":"cargo test"}],"reason":"run tests"}}"#));
        assert!(auto.is_none());
        let Ev::Approval(a) = &evs[0] else { panic!() };
        assert_eq!(a.command.as_deref(), Some("cargo test"));
        assert_eq!(a.request_id, "0");
        assert!(!a.read_only);
        let (raw, kind) = p.pending.lock().unwrap().remove("0").unwrap();
        assert_eq!(raw, json!(0));
        assert_eq!(decision_result(&kind, &Decision::Allow), json!({"decision":"accept"}));
        assert_eq!(decision_result(&kind, &Decision::AllowForSession), json!({"decision":"acceptForSession"}));
        assert_eq!(decision_result(&kind, &Decision::Deny { message: None }), json!({"decision":"decline"}));
        assert_eq!(decision_result(&kind, &Decision::Abort), json!({"decision":"cancel"}));
    }

    #[test]
    fn file_change_approval_uses_item_paths() {
        let mut p = Parser::new("C:\\w");
        p.feed(&v(r#"{"method":"item/started","params":{"threadId":"t","turnId":"u","startedAtMs":1,"item":{"type":"fileChange","id":"fc1","status":"inProgress","changes":[{"path":"C:\\w\\src\\a.rs","kind":{"type":"update"},"diff":"@@ -1 +1 @@\n-a\n+b"}]}}}"#));
        let (evs, _) = p.feed(&v(
            r#"{"id":7,"method":"item/fileChange/requestApproval","params":{"itemId":"fc1","threadId":"t","turnId":"u","startedAtMs":1}}"#,
        ));
        let Ev::Approval(a) = &evs[0] else { panic!() };
        assert_eq!(a.kind, ApprovalKind::FileChange);
        assert_eq!(a.paths, vec!["src/a.rs".to_string()]);
        assert!(a.preview.as_deref().unwrap().contains("+b"));
    }

    #[test]
    fn unsupported_server_requests_are_answered_not_hung() {
        let mut p = Parser::new(".");
        let (_, auto) = p.feed(&v(r#"{"id":3,"method":"item/tool/requestUserInput","params":{}}"#));
        assert_eq!(auto.unwrap().id, json!(3));
    }

    #[test]
    fn streaming_turn_and_usage() {
        let mut p = Parser::new(".");
        p.feed(&v(r#"{"method":"turn/started","params":{"threadId":"t","turn":{"id":"turn_1","items":[],"status":"inProgress"}}}"#));
        assert_eq!(p.turn.lock().unwrap().as_deref(), Some("turn_1"));
        let (d, _) = p
            .feed(&v(r#"{"method":"item/agentMessage/delta","params":{"threadId":"t","turnId":"turn_1","itemId":"msg_1","delta":"Hel"}}"#));
        assert!(matches!(&d[0], Ev::TextDelta { id, text } if id == "msg_1" && text == "Hel"));
        let (f, _) = p.feed(&v(r#"{"method":"item/completed","params":{"threadId":"t","turnId":"turn_1","item":{"type":"agentMessage","id":"msg_1","text":"Hello"}}}"#));
        assert!(matches!(&f[0], Ev::Text { text, .. } if text == "Hello"));
        let (u, _) = p.feed(&v(r#"{"method":"thread/tokenUsage/updated","params":{"threadId":"t","turnId":"turn_1","tokenUsage":{"last":{"totalTokens":1200,"inputTokens":1000,"outputTokens":200,"cachedInputTokens":0,"reasoningOutputTokens":0},"total":{"totalTokens":5000,"inputTokens":4000,"outputTokens":1000,"cachedInputTokens":3000,"reasoningOutputTokens":0},"modelContextWindow":272000}}}"#));
        assert!(matches!(u[0], Ev::Usage { context_tokens: Some(1200), context_window: Some(272_000), cached: 3000, cost_usd: None, .. }));
        let (c, _) =
            p.feed(&v(r#"{"method":"turn/completed","params":{"threadId":"t","turn":{"id":"turn_1","items":[],"status":"interrupted"}}}"#));
        assert!(matches!(c[0], Ev::TurnDone { outcome: TurnOutcome::Interrupted, .. }));
        assert!(p.turn.lock().unwrap().is_none(), "no turn left to interrupt");
    }

    #[test]
    fn command_items_and_plans() {
        let mut p = Parser::new("C:\\w");
        let (s, _) = p.feed(&v(r#"{"method":"item/started","params":{"item":{"type":"commandExecution","id":"c1","command":"rg foo","cwd":"C:\\w","status":"inProgress","commandActions":[{"type":"search","command":"rg foo","query":"foo","path":"src"}]}}}"#));
        assert!(matches!(&s[0], Ev::ToolStarted { kind: ToolKind::Search, label, .. } if label == "Searched for \"foo\" in src"));
        let (d, _) = p.feed(&v(r#"{"method":"item/completed","params":{"item":{"type":"commandExecution","id":"c1","command":"rg foo","status":"completed","exitCode":1,"aggregatedOutput":""}}}"#));
        assert!(d.iter().any(|e| matches!(e, Ev::ToolDone { status: ToolStatus::Failed, .. })));
        let (pl, _) = p.feed(&v(r#"{"method":"turn/plan/updated","params":{"threadId":"t","turnId":"u","plan":[{"step":"Read","status":"completed"},{"step":"Fix","status":"inProgress"}]}}"#));
        assert!(matches!(&pl[0], Ev::Plan(s) if s[1].status == StepStatus::InProgress));
    }

    #[test]
    fn account_models_limits_and_policies() {
        assert_eq!(
            parse_account(&json!({"account":{"type":"chatgpt","email":"a@b","planType":"plus"},"requiresOpenaiAuth":true})),
            AuthState::SignedIn { method: "ChatGPT".into(), account: Some("a@b".into()), plan: Some("plus".into()) }
        );
        assert_eq!(parse_account(&json!({"account":null,"requiresOpenaiAuth":true})), AuthState::SignedOut);
        assert_eq!(
            parse_login_status(true, "Logged in using ChatGPT"),
            AuthState::SignedIn { method: "ChatGPT".into(), account: None, plan: None }
        );
        assert_eq!(parse_login_status(false, "Not logged in"), AuthState::SignedOut);
        let m = parse_models(
            &json!({"data":[{"id":"x","model":"gpt-5.5-codex","displayName":"GPT-5.5 Codex","hidden":false,"isDefault":true,"defaultReasoningEffort":"medium","supportedReasoningEfforts":[{"reasoningEffort":"low","description":""},{"reasoningEffort":"high","description":""}],"description":""},{"id":"h","model":"hidden","hidden":true}]}),
        );
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].efforts, vec!["low", "high"]);
        let rl = rate_limits(
            &json!({"primary":{"usedPercent":42,"windowDurationMins":300,"resetsAt":1},"secondary":{"usedPercent":10,"windowDurationMins":10080}}),
        );
        assert_eq!(rl[0].label, "5-hour window");
        assert_eq!(rl[1].label, "7-day window");
        assert_eq!(policy("Supervised").0, "untrusted");
        assert_eq!(policy("Full access").1, json!({"type":"dangerFullAccess"}));
    }
}
