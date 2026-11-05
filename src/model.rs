//! Domain types shared by the store, the provider layer and the UI.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}
pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}
pub fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ProviderKind {
    Claude,
    Codex,
}

impl ProviderKind {
    pub const ALL: [ProviderKind; 2] = [ProviderKind::Claude, ProviderKind::Codex];
    pub fn label(self) -> &'static str {
        match self {
            ProviderKind::Claude => "Claude Code",
            ProviderKind::Codex => "Codex",
        }
    }
    pub fn short(self) -> &'static str {
        match self {
            ProviderKind::Claude => "CC",
            ProviderKind::Codex => "CX",
        }
    }
    pub fn key(self) -> &'static str {
        match self {
            ProviderKind::Claude => "claude",
            ProviderKind::Codex => "codex",
        }
    }
    pub fn from_key(s: &str) -> Option<Self> {
        match s {
            "claude" => Some(Self::Claude),
            "codex" => Some(Self::Codex),
            _ => None,
        }
    }
}

/// Run state of a thread. Persisted as its `key()`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RunState {
    Idle,
    Queued,
    Working,
    NeedsApproval,
    ReadyToReview,
    Completed,
    Failed,
    Cancelled,
}

impl RunState {
    pub fn key(self) -> &'static str {
        match self {
            RunState::Idle => "idle",
            RunState::Queued => "queued",
            RunState::Working => "working",
            RunState::NeedsApproval => "approval",
            RunState::ReadyToReview => "review",
            RunState::Completed => "completed",
            RunState::Failed => "failed",
            RunState::Cancelled => "cancelled",
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            RunState::Idle => "Idle",
            RunState::Queued => "Queued",
            RunState::Working => "Working",
            RunState::NeedsApproval => "Approval",
            RunState::ReadyToReview => "Review",
            RunState::Completed => "Done",
            RunState::Failed => "Failed",
            RunState::Cancelled => "Stopped",
        }
    }
    /// A run is "live" while the provider process owns the thread.
    pub fn is_active(self) -> bool {
        matches!(self, RunState::Queued | RunState::Working | RunState::NeedsApproval)
    }
    /// After a crash or restart nothing is running; never pretend it finished.
    pub fn after_restart(self) -> Self {
        if self.is_active() { RunState::Cancelled } else { self }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub pinned: bool,
    pub archived: bool,
    pub created_at: i64,
    pub last_used_at: i64,
    /// Project scripts shown in Thread details (name, command).
    #[serde(default)]
    pub scripts: Vec<(String, String)>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Usage {
    /// Provider-reported token counts for the latest turn context.
    pub context_tokens: Option<u64>,
    pub context_window: Option<u64>,
    pub total_input: u64,
    pub total_output: u64,
    pub cached_input: u64,
    /// Claude reports `total_cost_usd`: an API-equivalent estimate, not a subscription charge.
    pub est_cost_usd: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Thread {
    pub id: String,
    pub project_id: Option<String>,
    pub title: String,
    pub provider: ProviderKind,
    pub model: String,
    pub effort: Option<String>,
    pub permissions: String,
    pub state: RunState,
    /// Where the agent runs: the project checkout or a Kiln worktree.
    pub cwd: Option<PathBuf>,
    pub is_worktree: bool,
    pub branch: Option<String>,
    pub base_branch: Option<String>,
    pub base_rev: Option<String>,
    /// Provider-native session/thread id (Claude session_id, Codex thread id). Never a Kiln id.
    pub native_session_id: Option<String>,
    pub pinned: bool,
    pub archived: bool,
    pub settled: bool,
    pub compare_group: Option<String>,
    pub run_label: Option<String>,
    pub additions: u32,
    pub deletions: u32,
    pub files_changed: u32,
    pub created_at: i64,
    pub updated_at: i64,
    pub last_activity: Option<String>,
    pub turn_started_at: Option<i64>,
    pub worked_secs: i64,
    pub usage: Usage,
    pub plan: Vec<PlanStep>,
    pub last_error: Option<String>,
    pub queued: Vec<String>,
    pub test_summary: Option<String>,
    #[serde(default)]
    pub test_ok: Option<bool>,
    #[serde(default)]
    pub pr_url: Option<String>,
    #[serde(default)]
    pub kept: bool,
    /// Permission mode the provider itself reported (e.g. Claude's `permissionMode`).
    #[serde(default)]
    pub provider_mode: Option<String>,
    #[serde(default)]
    pub commit_message: Option<String>,
}

impl Thread {
    pub fn new(project_id: Option<String>, title: String, provider: ProviderKind, model: String) -> Self {
        let t = now();
        Self {
            id: new_id(),
            project_id,
            title,
            provider,
            model,
            effort: None,
            permissions: "Supervised".into(),
            state: RunState::Idle,
            cwd: None,
            is_worktree: false,
            branch: None,
            base_branch: None,
            base_rev: None,
            native_session_id: None,
            pinned: false,
            archived: false,
            settled: false,
            compare_group: None,
            run_label: None,
            additions: 0,
            deletions: 0,
            files_changed: 0,
            created_at: t,
            updated_at: t,
            last_activity: None,
            turn_started_at: None,
            worked_secs: 0,
            usage: Usage::default(),
            plan: Vec::new(),
            last_error: None,
            queued: Vec::new(),
            test_summary: None,
            test_ok: None,
            pr_url: None,
            kept: false,
            provider_mode: None,
            commit_message: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum StepStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanStep {
    pub step: String,
    pub status: StepStatus,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolStatus {
    Running,
    Ok,
    Failed,
    Declined,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolKind {
    Read,
    Search,
    List,
    Command,
    Edit,
    Web,
    Mcp,
    Task,
    Other,
}

/// One entry in a thread's conversation history.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum ItemKind {
    User {
        text: String,
        #[serde(default)]
        attachments: Vec<String>,
    },
    Assistant {
        text: String,
    },
    Reasoning {
        text: String,
    },
    Tool {
        tool_id: String,
        kind: ToolKind,
        label: String,
        detail: Option<String>,
        output: Option<String>,
        status: ToolStatus,
        #[serde(default)]
        paths: Vec<String>,
    },
    Decision {
        label: String,
        allowed: bool,
    },
    Error {
        text: String,
    },
    Notice {
        text: String,
    },
    TurnEnd {
        secs: i64,
        files: u32,
        additions: u32,
        deletions: u32,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Item {
    pub id: String,
    pub thread_id: String,
    pub seq: i64,
    pub created_at: i64,
    pub kind: ItemKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuleDecision {
    Allow,
    Block,
}

/// A Kiln-side approval rule. Kiln enforces it by answering the provider's own permission
/// request automatically; the provider still enforces the answer.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApprovalRule {
    pub id: String,
    pub project_id: Option<String>,
    pub pattern: String,
    pub decision: RuleDecision,
    pub created_at: i64,
    pub uses: u32,
    pub builtin: bool,
}

impl ApprovalRule {
    /// Glob-ish match: `*` matches any run of characters; whitespace is normalised.
    /// Allow rules never match commands that chain or redirect, so `cargo test *` can't
    /// approve `cargo test && rm -rf ~`. Block rules match any segment of a chained command.
    pub fn matches(&self, command: &str) -> bool {
        let pat = normalize_ws(&self.pattern);
        match self.decision {
            RuleDecision::Allow => !has_shell_chaining(command) && glob_match(&pat, &normalize_ws(command)),
            RuleDecision::Block => command.split(['&', '|', ';', '\n']).any(|seg| glob_match(&pat, &normalize_ws(seg))),
        }
    }
}

pub fn normalize_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn glob_match(pattern: &str, text: &str) -> bool {
    let (p, t): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    let (mut pi, mut ti, mut star, mut mark) = (0usize, 0usize, None::<usize>, 0usize);
    while ti < t.len() {
        if pi < p.len() && p[pi] == t[ti] {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

/// Shell metacharacters that would let a matched prefix smuggle a second command.
pub fn has_shell_chaining(cmd: &str) -> bool {
    ["&&", "||", ";", "|", "`", "$(", ">", "<", "\n"].iter().any(|m| cmd.contains(m))
}

/// Suggest a narrow rule for a command: the first two words plus `*`.
pub fn suggest_rule(command: &str) -> Option<String> {
    let c = normalize_ws(command);
    if c.is_empty() || has_shell_chaining(&c) {
        return None;
    }
    let words: Vec<&str> = c.split(' ').collect();
    Some(match words.len() {
        1 => words[0].to_string(),
        2 => c.clone(),
        _ => format!("{} {} *", words[0], words[1]),
    })
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReviewComment {
    pub id: String,
    pub thread_id: String,
    pub path: String,
    pub line: u32,
    /// True when `line` is in the old file (a deleted line).
    #[serde(default)]
    pub old_side: bool,
    /// Revision the comment was made against (base rev + working-tree snapshot sha).
    pub rev: String,
    pub text: String,
    pub sent: bool,
    pub resolved: bool,
    pub created_at: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Checkpoint {
    pub id: String,
    pub thread_id: String,
    pub sha: String,
    pub title: String,
    pub detail: String,
    pub additions: u32,
    pub deletions: u32,
    pub created_at: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_rules() {
        assert!(glob_match("cargo test *", "cargo test --features npcap capture::"));
        assert!(!glob_match("cargo test *", "cargo build"));
        assert!(glob_match("rm -rf *", "rm -rf /"));
        assert!(glob_match("git status", "git status"));
    }

    #[test]
    fn allow_rules_reject_chaining_block_rules_catch_it() {
        let mk = |p: &str, d| ApprovalRule {
            id: "r".into(),
            project_id: None,
            pattern: p.into(),
            decision: d,
            created_at: 0,
            uses: 0,
            builtin: false,
        };
        let allow = mk("cargo test *", RuleDecision::Allow);
        assert!(allow.matches("cargo test -p kiln"));
        assert!(!allow.matches("cargo test && rm -rf ~"));
        assert!(!allow.matches("cargo test > out.txt"));
        let block = mk("rm -rf *", RuleDecision::Block);
        assert!(block.matches("cargo test && rm -rf ~"));
        assert!(!block.matches("cargo test"));
    }

    #[test]
    fn rules_never_match_chained_commands_by_suggestion() {
        assert_eq!(suggest_rule("cargo test --features npcap capture::").as_deref(), Some("cargo test *"));
        assert_eq!(suggest_rule("cargo test && rm -rf ~"), None);
    }

    #[test]
    fn interrupted_runs_are_not_successful_after_restart() {
        for s in [RunState::Working, RunState::NeedsApproval, RunState::Queued] {
            assert_eq!(s.after_restart(), RunState::Cancelled);
        }
        assert_eq!(RunState::Completed.after_restart(), RunState::Completed);
    }
}
