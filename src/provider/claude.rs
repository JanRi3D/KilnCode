//! Claude Code adapter.
//!
//! Runs the installed CLI the same way the Claude Agent SDK does (verified against
//! Claude Code 2.1.240): `claude -p --input-format stream-json --output-format stream-json
//! --verbose --include-partial-messages --permission-prompt-tool stdio`.
//! Permission prompts arrive as `control_request{subtype:"can_use_tool"}` on stdout and are
//! answered with `control_response` on stdin; the CLI enforces the answer.
//! Authentication is whatever the user configured for Claude Code (`apiKeySource: "none"`
//! means their claude.ai subscription login). Kiln never passes `--bare` or an API key.

use super::process::{self, Managed};
use super::*;
use crate::model::{PlanStep, StepStatus, ToolKind, ToolStatus};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

pub fn parse_auth_status(out: &str) -> Option<AuthState> {
    let v: Value = serde_json::from_str(out.trim()).ok()?;
    if v.get("loggedIn").and_then(Value::as_bool) == Some(true) {
        let method = match str_field(&v, "authMethod") {
            Some("claude.ai") => "claude.ai".to_string(),
            Some(m) => m.to_string(),
            None => str_field(&v, "apiProvider").unwrap_or("Claude").to_string(),
        };
        Some(AuthState::SignedIn {
            method,
            account: str_field(&v, "email").map(str::to_string),
            plan: str_field(&v, "subscriptionType").map(str::to_string),
        })
    } else {
        Some(AuthState::SignedOut)
    }
}

/// The CLI has no model-list command; these are its documented aliases.
pub fn model_aliases() -> Vec<ModelInfo> {
    let efforts: Vec<String> = ["low", "medium", "high", "xhigh", "max"].iter().map(|s| s.to_string()).collect();
    [
        ("default", "Default (your Claude Code setting)", true),
        ("opus", "Claude Opus (latest)", false),
        ("sonnet", "Claude Sonnet (latest)", false),
        ("haiku", "Claude Haiku (latest)", false),
    ]
    .into_iter()
    .map(|(id, display, d)| ModelInfo {
        id: id.into(),
        display: display.into(),
        efforts: efforts.clone(),
        default_effort: Some("high".into()),
        is_default: d,
    })
    .collect()
}

pub fn permission_mode(preset: &str) -> &'static str {
    match preset {
        "Auto-accept edits" => "acceptEdits",
        "Plan (read-only)" => "plan",
        "Full access" => "bypassPermissions",
        _ => "default",
    }
}

pub fn build_args(cfg: &SessionConfig) -> Vec<String> {
    let mut a: Vec<String> = [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--permission-prompt-tool",
        "stdio",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    if let Some(m) = cfg.model.as_deref().filter(|m| !m.is_empty() && *m != "default") {
        a.extend(["--model".into(), m.into()]);
    }
    if let Some(e) = cfg.effort.as_deref().filter(|e| !e.is_empty()) {
        a.extend(["--effort".into(), e.into()]);
    }
    let mode = permission_mode(&cfg.permissions);
    if mode == "bypassPermissions" {
        a.push("--allow-dangerously-skip-permissions".into());
    }
    a.extend(["--permission-mode".into(), mode.into()]);
    if let Some(r) = cfg.resume.as_deref().filter(|r| !r.is_empty()) {
        a.extend(["--resume".into(), r.into()]);
    }
    a
}

/// Pending permission requests: request_id → (tool input, session-scoped suggestions).
pub type Pending = Arc<Mutex<HashMap<String, (Value, Vec<Value>)>>>;

pub struct Parser {
    pub cwd: String,
    cur_msg: String,
    cur_index: u64,
    streamed: HashSet<String>,
    tools: HashMap<String, ToolKind>,
    last_ctx: Option<u64>,
    pub pending: Pending,
    pub interrupted: Arc<Mutex<bool>>,
}

impl Parser {
    pub fn new(cwd: &str) -> Self {
        Self {
            cwd: cwd.to_string(),
            cur_msg: String::new(),
            cur_index: 0,
            streamed: HashSet::new(),
            tools: HashMap::new(),
            last_ctx: None,
            pending: Arc::default(),
            interrupted: Arc::default(),
        }
    }

    fn rel(&self, p: &str) -> String {
        rel_path(&self.cwd, p)
    }

    pub fn feed(&mut self, line: &str, complete: bool) -> Vec<Ev> {
        let v: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                return vec![if complete {
                    Ev::Log(format!("unparseable record from claude: {e}: {}", short(line, 200)))
                } else {
                    Ev::Error { message: "Claude Code's output ended in the middle of a message.".into(), will_retry: false }
                }];
            }
        };
        let mut out = Vec::new();
        match str_field(&v, "type").unwrap_or("") {
            "system" => match str_field(&v, "subtype") {
                Some("init") => out.push(Ev::Started {
                    native_id: str_field(&v, "session_id").unwrap_or_default().to_string(),
                    model: str_field(&v, "model").map(str::to_string),
                    permission_mode: str_field(&v, "permissionMode").map(str::to_string),
                    auth_source: str_field(&v, "apiKeySource").map(str::to_string),
                }),
                Some("compact_boundary") => out.push(Ev::Log("Claude Code compacted the conversation context.".into())),
                _ => {}
            },
            "stream_event" => self.stream_event(&v["event"], &mut out),
            "assistant" => self.assistant(&v["message"], &mut out),
            "user" => self.user(&v["message"], &mut out),
            "control_request" => self.control_request(&v, &mut out),
            "control_cancel_request" => {
                if let Some(id) = str_field(&v, "request_id") {
                    self.pending.lock().map(|mut p| p.remove(id)).ok();
                    out.push(Ev::ApprovalGone { request_id: id.to_string() });
                }
            }
            "control_response" => {
                let r = &v["response"];
                if str_field(r, "subtype") == Some("error") {
                    out.push(Ev::Log(format!("claude control error: {}", str_field(r, "error").unwrap_or("unknown"))));
                }
            }
            "rate_limit_event" => {
                let i = &v["rate_limit_info"];
                out.push(Ev::RateLimit(RateLimit {
                    label: str_field(i, "rateLimitType").unwrap_or("limit").replace('_', " "),
                    used_percent: i.get("utilization").and_then(Value::as_f64).map(|u| (u * 100.0) as f32),
                    resets_at: i.get("resetsAt").and_then(Value::as_i64),
                    status: str_field(i, "status").map(str::to_string),
                }));
            }
            "result" => self.result(&v, &mut out),
            _ => {}
        }
        out
    }

    fn stream_event(&mut self, e: &Value, out: &mut Vec<Ev>) {
        match str_field(e, "type").unwrap_or("") {
            "message_start" => {
                self.cur_msg = e["message"]["id"].as_str().unwrap_or_default().to_string();
            }
            "content_block_start" => {
                self.cur_index = e["index"].as_u64().unwrap_or(0);
            }
            "content_block_delta" => {
                let idx = e["index"].as_u64().unwrap_or(self.cur_index);
                let id = format!("{}:{}", self.cur_msg, idx);
                let d = &e["delta"];
                match str_field(d, "type") {
                    Some("text_delta") => {
                        self.streamed.insert(id.clone());
                        out.push(Ev::TextDelta { id, text: str_field(d, "text").unwrap_or_default().to_string() });
                    }
                    Some("thinking_delta") => {
                        self.streamed.insert(id.clone());
                        out.push(Ev::ReasoningDelta { id, text: str_field(d, "thinking").unwrap_or_default().to_string() });
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    fn assistant(&mut self, m: &Value, out: &mut Vec<Ev>) {
        let msg_id = str_field(m, "id").unwrap_or_default().to_string();
        if let Some(u) = m.get("usage") {
            let ctx = ["input_tokens", "cache_read_input_tokens", "cache_creation_input_tokens"]
                .iter()
                .filter_map(|k| u[*k].as_u64())
                .sum::<u64>();
            if ctx > 0 {
                self.last_ctx = Some(ctx);
            }
        }
        let blocks = m["content"].as_array().cloned().unwrap_or_default();
        let single = blocks.len() == 1;
        for (pos, b) in blocks.iter().enumerate() {
            let idx = if single && msg_id == self.cur_msg { self.cur_index } else { pos as u64 };
            let id = format!("{msg_id}:{idx}");
            match str_field(b, "type").unwrap_or("") {
                "text" => out.push(Ev::Text { id, text: str_field(b, "text").unwrap_or_default().to_string() }),
                "thinking" if !self.streamed.contains(&id) => {
                    out.push(Ev::ReasoningDelta { id, text: str_field(b, "thinking").unwrap_or_default().to_string() })
                }
                "tool_use" => {
                    let tid = str_field(b, "id").unwrap_or_default().to_string();
                    let name = str_field(b, "name").unwrap_or("tool");
                    let input = &b["input"];
                    if name == "TodoWrite" {
                        out.push(Ev::Plan(todos_to_plan(input)));
                    }
                    let (kind, label, detail, paths) = self.describe_tool(name, input);
                    self.tools.insert(tid.clone(), kind);
                    out.push(Ev::ToolStarted { id: tid, kind, label, detail, paths });
                }
                _ => {}
            }
        }
    }

    fn user(&mut self, m: &Value, out: &mut Vec<Ev>) {
        let Some(blocks) = m["content"].as_array() else { return };
        for b in blocks {
            if str_field(b, "type") != Some("tool_result") {
                continue;
            }
            let id = str_field(b, "tool_use_id").unwrap_or_default().to_string();
            let is_err = b["is_error"].as_bool().unwrap_or(false);
            let text = match &b["content"] {
                Value::String(s) => s.clone(),
                Value::Array(a) => a.iter().filter_map(|x| str_field(x, "text")).collect::<Vec<_>>().join("\n"),
                _ => String::new(),
            };
            let declined = is_err && (text.contains("doesn't want to proceed") || text.contains("declined") || text.contains("rejected"));
            let status = if declined {
                ToolStatus::Declined
            } else if is_err {
                ToolStatus::Failed
            } else {
                ToolStatus::Ok
            };
            if matches!(self.tools.get(&id), Some(ToolKind::Edit | ToolKind::Command)) {
                out.push(Ev::FilesTouched);
            }
            out.push(Ev::ToolDone { id, status, output: Some(truncate(&text, 8000)) });
        }
    }

    fn control_request(&mut self, v: &Value, out: &mut Vec<Ev>) {
        let req = &v["request"];
        if str_field(req, "subtype") != Some("can_use_tool") {
            return;
        }
        let request_id = str_field(v, "request_id").unwrap_or_default().to_string();
        let tool = str_field(req, "tool_name").unwrap_or("tool").to_string();
        let input = req["input"].clone();
        // Only session-scoped suggestions are offered: they never write the user's Claude settings files.
        let session: Vec<Value> = req["permission_suggestions"]
            .as_array()
            .map(|a| a.iter().filter(|s| str_field(s, "destination") == Some("session")).cloned().collect())
            .unwrap_or_default();
        let session_option = session.first().map(describe_suggestion);
        let mut a = ApprovalRequest {
            request_id: request_id.clone(),
            kind: ApprovalKind::Tool,
            title: format!("Use {tool}?"),
            tool_name: Some(tool.clone()),
            command: None,
            cwd: Some(self.cwd.clone()),
            paths: Vec::new(),
            reason: str_field(req, "decision_reason").or_else(|| str_field(&input, "description")).map(str::to_string),
            preview: None,
            read_only: matches!(tool.as_str(), "Read" | "Glob" | "Grep" | "LS"),
            network: matches!(tool.as_str(), "WebFetch" | "WebSearch"),
            session_option,
        };
        match tool.as_str() {
            "Bash" | "PowerShell" => {
                a.kind = ApprovalKind::Command;
                a.title = "Run this command?".into();
                a.command = str_field(&input, "command").map(str::to_string);
            }
            "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => {
                a.kind = ApprovalKind::FileChange;
                let path = str_field(&input, "file_path").or_else(|| str_field(&input, "notebook_path")).unwrap_or_default();
                let exists = std::path::Path::new(path).exists();
                a.title = if tool == "Write" && !exists { "Create this file?".into() } else { "Edit this file?".into() };
                a.paths.push(path.to_string());
                a.command = Some(self.rel(path));
                a.preview = Some(edit_preview(&tool, &input));
            }
            "WebFetch" => {
                a.kind = ApprovalKind::Network;
                a.title = "Fetch this URL?".into();
                a.command = str_field(&input, "url").map(str::to_string);
            }
            "WebSearch" => {
                a.kind = ApprovalKind::Network;
                a.title = "Search the web?".into();
                a.command = str_field(&input, "query").map(str::to_string);
            }
            _ => {
                a.command = Some(short(&input.to_string(), 300));
            }
        }
        if let Some(bp) = str_field(req, "blocked_path") {
            a.paths.push(bp.to_string());
        }
        if let Ok(mut p) = self.pending.lock() {
            p.insert(request_id, (input, session));
        }
        out.push(Ev::Approval(a));
    }

    fn result(&mut self, v: &Value, out: &mut Vec<Ev>) {
        let u = &v["usage"];
        let window = v["modelUsage"].as_object().and_then(|m| m.values().filter_map(|x| x["contextWindow"].as_u64()).max());
        out.push(Ev::Usage {
            context_tokens: self.last_ctx,
            context_window: window,
            input: u["input_tokens"].as_u64().unwrap_or(0) + u["cache_creation_input_tokens"].as_u64().unwrap_or(0),
            output: u["output_tokens"].as_u64().unwrap_or(0),
            cached: u["cache_read_input_tokens"].as_u64().unwrap_or(0),
            cost_usd: v["total_cost_usd"].as_f64(),
        });
        let interrupted = std::mem::take(&mut *self.interrupted.lock().unwrap_or_else(|e| e.into_inner()));
        let is_error = v["is_error"].as_bool().unwrap_or(false);
        let reason = str_field(v, "terminal_reason").unwrap_or("");
        let outcome = if interrupted || reason.contains("interrupt") || reason.contains("abort") {
            TurnOutcome::Interrupted
        } else if is_error {
            TurnOutcome::Failed
        } else {
            TurnOutcome::Completed
        };
        let error = (outcome == TurnOutcome::Failed).then(|| {
            str_field(v, "result")
                .map(str::to_string)
                .or_else(|| v["errors"].as_array().map(|e| e.iter().filter_map(Value::as_str).collect::<Vec<_>>().join("; ")))
                .unwrap_or_else(|| str_field(v, "subtype").unwrap_or("error").to_string())
        });
        out.push(Ev::TurnDone { outcome, error });
    }

    fn describe_tool(&self, name: &str, i: &Value) -> (ToolKind, String, Option<String>, Vec<String>) {
        let path =
            str_field(i, "file_path").or_else(|| str_field(i, "notebook_path")).or_else(|| str_field(i, "path")).map(|p| self.rel(p));
        let paths: Vec<String> = path.iter().cloned().collect();
        match name {
            "Read" => (ToolKind::Read, format!("Read {}", path.clone().unwrap_or_default()), None, paths),
            "Glob" => (ToolKind::Search, format!("Searched for files matching {}", str_field(i, "pattern").unwrap_or("")), None, paths),
            "Grep" => {
                let p = str_field(i, "pattern").unwrap_or("");
                let r#in = path.map(|p| format!(" in {p}")).unwrap_or_default();
                (ToolKind::Search, format!("Searched for \"{}\"{}", short(p, 60), r#in), None, Vec::new())
            }
            "LS" => (ToolKind::List, format!("Listed {}", path.unwrap_or_else(|| ".".into())), None, Vec::new()),
            "Bash" | "PowerShell" => {
                let c = str_field(i, "command").unwrap_or("");
                (ToolKind::Command, format!("Ran {}", short(c, 90)), Some(c.to_string()), Vec::new())
            }
            "Edit" | "MultiEdit" | "NotebookEdit" => (ToolKind::Edit, format!("Edited {}", path.unwrap_or_default()), None, paths),
            "Write" => (ToolKind::Edit, format!("Wrote {}", path.unwrap_or_default()), None, paths),
            "WebFetch" => (ToolKind::Web, format!("Fetched {}", str_field(i, "url").unwrap_or("")), None, Vec::new()),
            "WebSearch" => (ToolKind::Web, format!("Searched the web for \"{}\"", str_field(i, "query").unwrap_or("")), None, Vec::new()),
            "Task" | "Agent" => (
                ToolKind::Task,
                format!("Ran a subagent: {}", str_field(i, "description").unwrap_or("task")),
                str_field(i, "prompt").map(str::to_string),
                Vec::new(),
            ),
            "TodoWrite" => (ToolKind::Other, "Updated the plan".into(), None, Vec::new()),
            n if n.starts_with("mcp__") => {
                let mut parts = n.splitn(3, "__").skip(1);
                (
                    ToolKind::Mcp,
                    format!("Called {} · {}", parts.next().unwrap_or(""), parts.next().unwrap_or("")),
                    Some(short(&i.to_string(), 400)),
                    Vec::new(),
                )
            }
            n => (ToolKind::Other, format!("Used {n}"), Some(short(&i.to_string(), 400)), Vec::new()),
        }
    }
}

fn todos_to_plan(input: &Value) -> Vec<PlanStep> {
    input["todos"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|t| PlanStep {
                    step: str_field(t, "content").unwrap_or_default().to_string(),
                    status: match str_field(t, "status") {
                        Some("completed") => StepStatus::Completed,
                        Some("in_progress") => StepStatus::InProgress,
                        _ => StepStatus::Pending,
                    },
                })
                .collect()
        })
        .unwrap_or_default()
}

fn describe_suggestion(s: &Value) -> String {
    match str_field(s, "type") {
        Some("setMode") => match str_field(s, "mode") {
            Some("acceptEdits") => "Accept edits for this session".into(),
            Some(m) => format!("Switch to {m} for this session"),
            None => "Allow for this session".into(),
        },
        Some("addRules") => {
            let rules: Vec<String> = s["rules"]
                .as_array()
                .map(|r| {
                    r.iter()
                        .map(|x| match str_field(x, "ruleContent") {
                            Some(c) => format!("{}({c})", str_field(x, "toolName").unwrap_or("")),
                            None => str_field(x, "toolName").unwrap_or("").to_string(),
                        })
                        .collect()
                })
                .unwrap_or_default();
            format!("Always allow {} this session", rules.join(", "))
        }
        _ => "Allow for this session".into(),
    }
}

fn edit_preview(tool: &str, i: &Value) -> String {
    let mut out = String::new();
    let mut push = |prefix: &str, s: &str| {
        for l in s.lines().take(24) {
            out.push_str(prefix);
            out.push_str(l);
            out.push('\n');
        }
    };
    match tool {
        "Write" => push("+ ", str_field(i, "content").unwrap_or("")),
        "MultiEdit" => {
            for e in i["edits"].as_array().into_iter().flatten().take(4) {
                push("- ", str_field(e, "old_string").unwrap_or(""));
                push("+ ", str_field(e, "new_string").unwrap_or(""));
            }
        }
        _ => {
            push("- ", str_field(i, "old_string").unwrap_or(""));
            push("+ ", str_field(i, "new_string").or_else(|| str_field(i, "new_source")).unwrap_or(""));
        }
    }
    truncate(out.trim_end(), 4000)
}

pub fn rel_path(cwd: &str, p: &str) -> String {
    let norm = |s: &str| s.replace('\\', "/");
    let (c, q) = (norm(cwd), norm(p));
    let c = c.trim_end_matches('/');
    if !c.is_empty() && q.len() > c.len() && q[..c.len()].eq_ignore_ascii_case(c) && q.as_bytes()[c.len()] == b'/' {
        q[c.len() + 1..].to_string()
    } else {
        q
    }
}

pub fn truncate(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    let mut cut = n;
    while !s.is_char_boundary(cut) {
        cut -= 1;
    }
    format!("{}…", &s[..cut])
}

pub fn decision_response(request_id: &str, decision: &Decision, input: Value, session: Vec<Value>) -> Value {
    let body = match decision {
        Decision::Allow => json!({ "behavior": "allow", "updatedInput": input }),
        Decision::AllowForSession => json!({ "behavior": "allow", "updatedInput": input, "updatedPermissions": session }),
        Decision::Deny { message } => {
            json!({ "behavior": "deny", "message": message.clone().filter(|m| !m.trim().is_empty()).unwrap_or_else(|| "The user declined this request.".into()) })
        }
        Decision::Abort => {
            json!({ "behavior": "deny", "message": "The user declined this request and stopped the turn.", "interrupt": true })
        }
    };
    json!({ "type": "control_response", "response": { "subtype": "success", "request_id": request_id, "response": body } })
}

pub fn user_message(text: &str, attachments: &[std::path::PathBuf]) -> Value {
    let mut content = vec![];
    let mut files = Vec::new();
    for a in attachments {
        let ext = a.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        let media = match ext.as_str() {
            "png" => Some("image/png"),
            "jpg" | "jpeg" => Some("image/jpeg"),
            "gif" => Some("image/gif"),
            "webp" => Some("image/webp"),
            _ => None,
        };
        match (media, std::fs::read(a)) {
            (Some(m), Ok(bytes)) if bytes.len() < 5 * 1024 * 1024 => {
                content.push(json!({ "type": "image", "source": { "type": "base64", "media_type": m, "data": base64(&bytes) } }))
            }
            _ => files.push(a.display().to_string()),
        }
    }
    let mut t = text.to_string();
    if !files.is_empty() {
        t.push_str("\n\nAttached files:\n");
        for f in files {
            t.push_str(&format!("- {f}\n"));
        }
    }
    content.insert(0, json!({ "type": "text", "text": t }));
    json!({ "type": "user", "message": { "role": "user", "content": content }, "parent_tool_use_id": null, "session_id": "" })
}

pub fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        s.push(T[(n >> 18) as usize & 63] as char);
        s.push(T[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        s.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    s
}

pub fn start(cfg: SessionConfig, sink: Sink, rx: mpsc::Receiver<Cmd>) -> Result<(), String> {
    let mut cmd = process::command(&cfg.exe);
    cmd.args(build_args(&cfg)).current_dir(&cfg.cwd);
    crate::log::info(&format!("starting claude in {} (resume: {})", cfg.cwd.display(), cfg.resume.is_some()));
    let (m, stdout, stderr) = Managed::spawn(cmd)?;
    let tail = process::stderr_collector(stderr, sink.clone());
    let mut parser = Parser::new(&cfg.cwd.to_string_lossy());
    let pending = parser.pending.clone();
    let interrupted = parser.interrupted.clone();

    let reader_sink = sink.clone();
    let reader_m = m.clone();
    std::thread::Builder::new()
        .name("claude-reader".into())
        .spawn(move || {
            process::read_lines(stdout, |line, complete| {
                for ev in parser.feed(line, complete) {
                    reader_sink.emit(ev);
                }
            });
            let code = (0..100).find_map(|_| {
                let c = reader_m.try_exit_code();
                if c.is_none() {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
                c
            });
            let stderr_tail = tail.lock().map(|t| t.clone()).unwrap_or_default();
            reader_sink.emit(Ev::Exited { code: code.flatten(), stderr_tail });
        })
        .map_err(|e| e.to_string())?;

    let init = json!({ "type": "control_request", "request_id": format!("kiln-init-{}", crate::model::now_ms()), "request": { "subtype": "initialize", "hooks": null } });
    m.write_line(&init.to_string())?;

    std::thread::Builder::new()
        .name("claude-driver".into())
        .spawn(move || {
            for cmd in rx {
                let r = match cmd {
                    Cmd::Send { text, attachments, .. } => m.write_line(&user_message(&text, &attachments).to_string()),
                    Cmd::Interrupt => {
                        *interrupted.lock().unwrap_or_else(|e| e.into_inner()) = true;
                        let req = json!({ "type": "control_request", "request_id": format!("kiln-int-{}", crate::model::now_ms()), "request": { "subtype": "interrupt" } });
                        m.write_line(&req.to_string())
                    }
                    Cmd::Approve { request_id, decision } => {
                        let entry = pending.lock().ok().and_then(|mut p| p.remove(&request_id));
                        match entry {
                            Some((input, session)) => m.write_line(&decision_response(&request_id, &decision, input, session).to_string()),
                            None => Err("That request is no longer pending.".into()),
                        }
                    }
                    Cmd::Shutdown => break,
                };
                if let Err(e) = r {
                    sink.emit(Ev::Error { message: e, will_retry: false });
                }
            }
            m.shutdown(std::time::Duration::from_secs(3));
        })
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Records captured from Claude Code 2.1.240 (trimmed).
    const INIT: &str = r#"{"type":"system","subtype":"init","cwd":"C:\\w","session_id":"6cd6cff2-3606","model":"claude-haiku-4-5","permissionMode":"default","apiKeySource":"none","tools":["Bash"]}"#;
    const ASK: &str = r#"{"type":"control_request","request_id":"bfca7b2e","request":{"subtype":"can_use_tool","tool_name":"Write","display_name":"Write","input":{"file_path":"C:\\w\\probe.txt","content":"hi"},"description":"probe.txt","permission_suggestions":[{"type":"setMode","mode":"acceptEdits","destination":"session"},{"type":"addRules","rules":[{"toolName":"Write"}],"behavior":"allow","destination":"localSettings"}],"tool_use_id":"toolu_01"}}"#;

    #[test]
    fn init_yields_native_session_id_and_auth_source() {
        let mut p = Parser::new("C:\\w");
        match &p.feed(INIT, true)[0] {
            Ev::Started { native_id, auth_source, permission_mode, .. } => {
                assert_eq!(native_id, "6cd6cff2-3606");
                assert_eq!(auth_source.as_deref(), Some("none"));
                assert_eq!(permission_mode.as_deref(), Some("default"));
            }
            e => panic!("unexpected {e:?}"),
        }
    }

    #[test]
    fn streaming_text_and_final_message_share_an_item_id() {
        let mut p = Parser::new("C:\\w");
        p.feed(r#"{"type":"stream_event","event":{"type":"message_start","message":{"id":"msg_1"}}}"#, true);
        p.feed(r#"{"type":"stream_event","event":{"type":"content_block_start","index":1,"content_block":{"type":"text"}}}"#, true);
        let d = p.feed(
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"do"}}}"#,
            true,
        );
        let f = p.feed(r#"{"type":"assistant","message":{"id":"msg_1","content":[{"type":"text","text":"done."}],"usage":{"input_tokens":8,"cache_read_input_tokens":100,"cache_creation_input_tokens":2}}}"#, true);
        let (Ev::TextDelta { id: a, .. }, Ev::Text { id: b, text }) = (&d[0], &f[0]) else { panic!() };
        assert_eq!(a, b);
        assert_eq!(text, "done.");
        let r = p.feed(r#"{"type":"result","is_error":false,"total_cost_usd":0.07,"usage":{"input_tokens":18,"output_tokens":190,"cache_read_input_tokens":34878},"modelUsage":{"m":{"contextWindow":200000}},"terminal_reason":"completed"}"#, true);
        assert!(matches!(r[0], Ev::Usage { context_tokens: Some(110), context_window: Some(200_000), cost_usd: Some(_), .. }));
        assert!(matches!(r[1], Ev::TurnDone { outcome: TurnOutcome::Completed, .. }));
    }

    #[test]
    fn permission_request_routes_and_only_offers_session_scoped_rules() {
        let mut p = Parser::new("C:\\w");
        let evs = p.feed(ASK, true);
        let Ev::Approval(a) = &evs[0] else { panic!() };
        assert_eq!(a.kind, ApprovalKind::FileChange);
        assert_eq!(a.command.as_deref(), Some("probe.txt"));
        assert_eq!(a.session_option.as_deref(), Some("Accept edits for this session"));
        let (input, session) = p.pending.lock().unwrap().remove("bfca7b2e").unwrap();
        assert_eq!(session.len(), 1, "localSettings suggestion must not be offered");
        let allow = decision_response("bfca7b2e", &Decision::AllowForSession, input.clone(), session);
        assert_eq!(allow["response"]["response"]["behavior"], "allow");
        assert_eq!(allow["response"]["response"]["updatedPermissions"][0]["destination"], "session");
        assert_eq!(allow["response"]["request_id"], "bfca7b2e");
        let deny = decision_response("bfca7b2e", &Decision::Deny { message: Some("use tests/ instead".into()) }, input, vec![]);
        assert_eq!(deny["response"]["response"]["behavior"], "deny");
        assert_eq!(deny["response"]["response"]["message"], "use tests/ instead");
        let abort = decision_response("x", &Decision::Abort, json!({}), vec![]);
        assert_eq!(abort["response"]["response"]["interrupt"], true);
    }

    #[test]
    fn cancel_request_clears_pending_approval() {
        let mut p = Parser::new("C:\\w");
        p.feed(ASK, true);
        let evs = p.feed(r#"{"type":"control_cancel_request","request_id":"bfca7b2e"}"#, true);
        assert!(matches!(&evs[0], Ev::ApprovalGone { request_id } if request_id == "bfca7b2e"));
        assert!(p.pending.lock().unwrap().is_empty());
    }

    #[test]
    fn tools_map_to_activity_and_results() {
        let mut p = Parser::new("C:\\w");
        let evs = p.feed(r#"{"type":"assistant","message":{"id":"m2","content":[{"type":"tool_use","id":"tu1","name":"Bash","input":{"command":"cargo test","description":"Run tests"}}]}}"#, true);
        assert!(matches!(&evs[0], Ev::ToolStarted { kind: ToolKind::Command, label, .. } if label == "Ran cargo test"));
        let done = p.feed(
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"tu1","content":"ok","is_error":false}]}}"#,
            true,
        );
        assert!(done.iter().any(|e| matches!(e, Ev::ToolDone { status: ToolStatus::Ok, .. })));
        let todo = p.feed(r#"{"type":"assistant","message":{"id":"m3","content":[{"type":"tool_use","id":"tu2","name":"TodoWrite","input":{"todos":[{"content":"A","status":"completed"},{"content":"B","status":"in_progress"}]}}]}}"#, true);
        assert!(matches!(&todo[0], Ev::Plan(steps) if steps.len() == 2 && steps[1].status == StepStatus::InProgress));
    }

    #[test]
    fn truncated_and_failed_records() {
        let mut p = Parser::new("C:\\w");
        assert!(matches!(p.feed(r#"{"type":"assist"#, false)[0], Ev::Error { .. }));
        assert!(matches!(p.feed("not json", true)[0], Ev::Log(_)));
        let r = p.feed(r#"{"type":"result","is_error":true,"result":"API Error: overloaded","usage":{}}"#, true);
        assert!(matches!(&r[1], Ev::TurnDone { outcome: TurnOutcome::Failed, error: Some(e) } if e.contains("overloaded")));
        *p.interrupted.lock().unwrap() = true;
        let r = p.feed(r#"{"type":"result","is_error":true,"usage":{}}"#, true);
        assert!(matches!(&r[1], Ev::TurnDone { outcome: TurnOutcome::Interrupted, .. }));
    }

    #[test]
    fn args_preserve_subscription_auth_and_resume() {
        let cfg = SessionConfig {
            kind: crate::model::ProviderKind::Claude,
            exe: "claude".into(),
            cwd: ".".into(),
            model: Some("opus".into()),
            effort: Some("high".into()),
            permissions: "Supervised".into(),
            resume: Some("sess-1".into()),
            thread_id: "t".into(),
        };
        let a = build_args(&cfg).join(" ");
        assert!(a.contains("--permission-prompt-tool stdio"));
        assert!(a.contains("--resume sess-1"));
        assert!(a.contains("--permission-mode default"));
        assert!(!a.contains("--bare") && !a.contains("api-key") && !a.contains("dangerously"));
    }

    #[test]
    fn auth_status_and_paths() {
        let s = parse_auth_status(r#"{"loggedIn":true,"authMethod":"claude.ai","email":"a@b.c","subscriptionType":"max"}"#).unwrap();
        assert_eq!(s, AuthState::SignedIn { method: "claude.ai".into(), account: Some("a@b.c".into()), plan: Some("max".into()) });
        assert_eq!(parse_auth_status(r#"{"loggedIn":false}"#), Some(AuthState::SignedOut));
        assert_eq!(rel_path("C:\\w", "c:\\W\\src\\a.rs"), "src/a.rs");
        assert_eq!(base64(b"hello"), "aGVsbG8=");
    }
}
