//! Live end-to-end tests against the installed provider CLIs. They use the user's existing
//! provider login and cost a few small turns, so they're `#[ignore]`d:
//!   cargo test live_ -- --ignored --nocapture --test-threads=1
//! Fixture tests elsewhere don't prove provider integration; these do.

use crate::app::{App, View};
use crate::model::*;
use crate::provider::Decision;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn git(dir: &Path, args: &[&str]) -> String {
    let o = std::process::Command::new("git")
        .current_dir(dir)
        .args(["-c", "user.name=Kiln Test", "-c", "user.email=test@kiln.invalid", "-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .expect("git");
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

fn repo(dir: &Path) -> PathBuf {
    let r = dir.join("e2e-repo");
    std::fs::create_dir_all(&r).unwrap();
    git(&r, &["init", "-q", "-b", "main"]);
    // The test repo shouldn't trigger the user's global commit signing (e.g. a 1Password prompt).
    git(&r, &["config", "commit.gpgsign", "false"]);
    git(&r, &["config", "user.name", "Kiln Test"]);
    git(&r, &["config", "user.email", "test@kiln.invalid"]);
    std::fs::write(r.join("README.md"), "# e2e\n").unwrap();
    git(&r, &["add", "-A"]);
    git(&r, &["commit", "-q", "-m", "init"]);
    r
}

fn pump_until(app: &mut App, secs: u64, mut cond: impl FnMut(&mut App) -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(secs) {
        app.pump();
        if cond(app) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

fn transcript(app: &App, id: &str) -> String {
    app.items.get(id).map(|l| l.iter().map(|i| format!("{:?}", i.kind)).collect::<Vec<_>>().join("\n")).unwrap_or_default()
}

/// Approves (or declines) every pending request until the turn ends.
fn run_turn(app: &mut App, id: &str, approve: bool, secs: u64) -> Vec<String> {
    let mut seen = Vec::new();
    let done = pump_until(app, secs, |a| {
        if let Some(req) = a.approvals.get(id).and_then(|l| l.first()).cloned() {
            seen.push(format!("{:?}: {}", req.kind, req.command.clone().unwrap_or_default()));
            let d = if approve { Decision::Allow } else { Decision::Deny { message: Some("Declined by the e2e test.".into()) } };
            a.decide(id, &req.request_id, d, None);
        }
        let t = a.thread(id).unwrap();
        !t.state.is_active() && a.approvals.get(id).is_none_or(|l| l.is_empty())
    });
    assert!(done, "turn did not finish in {secs}s:\n{}", transcript(app, id));
    seen
}

fn setup(kind: ProviderKind, model: &str) -> (tempfile::TempDir, App, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let store = crate::store::Store::open(&dir.path().join("kiln.db")).unwrap();
    let mut app = App::new(egui::Context::default(), store, false);
    let mut s = app.settings.clone();
    s.worktree_location = dir.path().join("worktrees").display().to_string();
    s.risk_read = "Ask".into();
    app.settings = s;
    assert!(pump_until(&mut app, 60, |a| a.providers.get(&kind).is_some_and(|p| p.checked_at > 0)), "health check timed out");
    let st = app.providers.get(&kind).cloned().unwrap();
    println!("{} status: {} (version {:?})", kind.label(), st.summary(), st.version);
    assert!(st.ready(), "{} is not ready: {}", kind.label(), st.summary());
    let r = repo(dir.path());
    app.add_project(&r);
    app.draft.provider = kind;
    app.draft.model = model.into();
    app.draft.worktree = true;
    (dir, app, r)
}

fn start(app: &mut App, prompt: &str) -> String {
    app.draft.prompt = prompt.into();
    app.start_from_draft();
    assert_eq!(app.view, View::Thread);
    app.current.clone().unwrap()
}

#[test]
#[ignore]
fn live_claude_end_to_end() {
    let (dir, mut app, repo) = setup(ProviderKind::Claude, "haiku");
    app.draft.effort = "low".into();
    let id = start(
        &mut app,
        "Use the Write tool to create a file named kiln_e2e.txt containing exactly the word ok. Do nothing else, then reply with one word: done.",
    );
    let asked = run_turn(&mut app, &id, true, 240);
    let t = app.thread(&id).unwrap().clone();
    println!("approvals seen: {asked:?}\nstate {:?} branch {:?} native {:?}", t.state, t.branch, t.native_session_id);
    assert!(!asked.is_empty(), "Supervised mode should have asked before writing:\n{}", transcript(&app, &id));
    let cwd = t.cwd.clone().unwrap();
    assert!(cwd.starts_with(dir.path().join("worktrees")), "runs in a Kiln worktree, not the checkout");
    assert!(t.branch.as_deref().unwrap().starts_with("kiln/"));
    assert_eq!(std::fs::read_to_string(cwd.join("kiln_e2e.txt")).unwrap().trim(), "ok");
    assert!(!repo.join("kiln_e2e.txt").exists(), "the user's checkout is untouched");
    let native = t.native_session_id.clone().expect("Claude session id recorded");
    assert_ne!(native, t.id);
    // Changes are visible through git.
    app.refresh_git(&id);
    assert!(pump_until(&mut app, 30, |a| a.git.get(&id).is_some_and(|g| !g.loading && !g.changes.is_empty())));
    assert_eq!(app.git[&id].changes[0].path, "kiln_e2e.txt");
    assert!(app.thread(&id).unwrap().usage.est_cost_usd.is_some(), "Claude reports an API-equivalent estimate");

    // Decline is enforced by the provider: the file must not appear.
    app.send(
        &id,
        "Use the Write tool to create a file named denied.txt containing no. If you are not allowed, just reply: skipped.".into(),
        false,
    );
    let asked = run_turn(&mut app, &id, false, 240);
    println!("declined: {asked:?}");
    assert!(!asked.is_empty());
    assert!(!cwd.join("denied.txt").exists(), "declined write was not performed");
    assert_eq!(app.thread(&id).unwrap().native_session_id.as_deref(), Some(native.as_str()), "same live session");

    // Restart Kiln: the thread resumes the provider's own session.
    let db = dir.path().join("kiln.db");
    app.shutdown();
    drop(app);
    let store = crate::store::Store::open(&db).unwrap();
    let mut app = App::new(egui::Context::default(), store, false);
    app.open_thread(&id);
    assert!(!app.thread(&id).unwrap().state.is_active());
    app.send(&id, "What is the exact file name you created earlier in this conversation? Reply with just the file name.".into(), false);
    run_turn(&mut app, &id, true, 240);
    let reply = app.items[&id]
        .iter()
        .rev()
        .find_map(|i| if let ItemKind::Assistant { text } = &i.kind { Some(text.clone()) } else { None })
        .unwrap_or_default();
    println!("resumed reply: {reply}");
    assert!(reply.contains("kiln_e2e.txt"), "resumed session remembers context: {reply}");

    // Commit (no push: the temp repo has no remote).
    app.commit_and_push(&id, "Add kiln_e2e.txt".into(), false);
    let ok = pump_until(&mut app, 60, |a| a.toasts.iter().any(|t| t.text.contains("Commit")));
    let toasts: Vec<String> = app.toasts.iter().map(|t| t.text.clone()).collect();
    assert!(ok && toasts.iter().any(|t| t.contains("Committed")), "commit result: {toasts:?}");
    assert_eq!(git(&cwd, &["log", "-1", "--format=%s"]), "Add kiln_e2e.txt");
    app.shutdown();
}

#[test]
#[ignore]
fn live_codex_end_to_end() {
    let (_dir, mut app, repo) = setup(ProviderKind::Codex, "");
    let id = start(
        &mut app,
        "Create a file named kiln_e2e.txt containing exactly the word ok. Do nothing else, then reply with one word: done.",
    );
    let asked = run_turn(&mut app, &id, true, 300);
    let t = app.thread(&id).unwrap().clone();
    println!("approvals seen: {asked:?}\nstate {:?} native {:?}\n{}", t.state, t.native_session_id, transcript(&app, &id));
    let cwd = t.cwd.clone().unwrap();
    assert!(t.native_session_id.is_some(), "Codex thread id recorded");
    assert_eq!(std::fs::read_to_string(cwd.join("kiln_e2e.txt")).unwrap_or_default().trim(), "ok", "file written in the worktree");
    assert!(!repo.join("kiln_e2e.txt").exists());
    // Follow-up in the same Codex thread.
    app.send(&id, "Reply with the exact name of the file you just created, nothing else.".into(), false);
    run_turn(&mut app, &id, true, 300);
    let reply = app.items[&id]
        .iter()
        .rev()
        .find_map(|i| if let ItemKind::Assistant { text } = &i.kind { Some(text.clone()) } else { None })
        .unwrap_or_default();
    println!("follow-up reply: {reply}");
    assert!(reply.contains("kiln_e2e.txt"));
    app.shutdown();
}
