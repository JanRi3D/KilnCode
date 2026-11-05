//! "From your repo" starters and local test runs. Everything here comes from the actual
//! repository (git grep, git status) or the user's GitHub CLI; nothing is invented.

use crate::app::Starter;
use crate::theme::hex;
use std::path::Path;
use std::process::Command;
use std::time::Duration;

fn run(cwd: &Path, prog: &str, args: &[&str], secs: u64) -> Option<String> {
    let mut c = Command::new(prog);
    c.current_dir(cwd).args(args).env("GIT_TERMINAL_PROMPT", "0").env("GH_PROMPT_DISABLED", "1").stdin(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    let o = crate::provider::process::output_with_timeout(c, Duration::from_secs(secs)).ok()?;
    o.status.success().then(|| String::from_utf8_lossy(&o.stdout).to_string())
}

pub fn starters(repo: &Path) -> Vec<Starter> {
    let mut out = Vec::new();
    // Failing CI on the default branch (GitHub Actions via gh), if available.
    let base = crate::git::default_base_branch(repo);
    if let Some(json) =
        run(repo, "gh", &["run", "list", "--branch", &base, "--limit", "1", "--json", "conclusion,workflowName,displayTitle,createdAt"], 15)
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(&json)
        && let Some(r) = v.as_array().and_then(|a| a.first()).filter(|r| r["conclusion"] == "failure")
    {
        let wf = r["workflowName"].as_str().unwrap_or("CI");
        out.push(Starter {
            tag: "Failing check".into(),
            tone: hex(0xff6467),
            title: format!("{wf} is failing on {base}"),
            meta: format!("GitHub Actions · {}", r["displayTitle"].as_str().unwrap_or("")),
            prompt: format!("The {wf} workflow is failing on {base}. Find the cause and fix it."),
        });
    }
    // Open GitHub issues.
    if let Some(json) = run(repo, "gh", &["issue", "list", "--limit", "2", "--json", "number,title,comments"], 15)
        && let Ok(v) = serde_json::from_str::<serde_json::Value>(&json)
    {
        for i in v.as_array().into_iter().flatten() {
            let n = i["number"].as_i64().unwrap_or(0);
            let title = i["title"].as_str().unwrap_or("").to_string();
            let c = i["comments"].as_array().map(|a| a.len()).unwrap_or(0);
            out.push(Starter {
                tag: format!("Issue #{n}"),
                tone: hex(0x51a2ff),
                title: title.clone(),
                meta: format!("GitHub · {c} comment{}", if c == 1 { "" } else { "s" }),
                prompt: format!("Implement issue #{n}: {title}"),
            });
        }
    }
    // TODO / FIXME comments in tracked files.
    if let Some(grep) = run(
        repo,
        "git",
        &["grep", "-n", "-I", "-E", "--max-count=1", r"(TODO|FIXME)[:( ]", "--", ".", ":!*.lock", ":!*.min.js", ":!*.svg"],
        15,
    ) {
        for line in grep.lines().take(3) {
            let mut parts = line.splitn(3, ':');
            let (Some(path), Some(no), Some(text)) = (parts.next(), parts.next(), parts.next()) else { continue };
            let note = text.split_once("TODO").or_else(|| text.split_once("FIXME")).map(|(_, r)| r).unwrap_or(text);
            let note = note.trim_start_matches([':', '(', ')', ' ']).trim().trim_end_matches("*/").trim();
            if note.is_empty() {
                continue;
            }
            let short: String = note.chars().take(70).collect();
            out.push(Starter {
                tag: "TODO".into(),
                tone: hex(0xffb900),
                title: short,
                meta: format!("{path}:{no}"),
                prompt: format!("Resolve the TODO in {path}:{no}: {note}"),
            });
        }
    }
    // Uncommitted local work.
    if let Ok(ch) = crate::git::status(repo)
        && !ch.is_empty()
    {
        let n = ch.len();
        out.push(Starter {
            tag: "Local changes".into(),
            tone: hex(0xa1a1a1),
            title: format!("{n} uncommitted file{}", if n == 1 { "" } else { "s" }),
            meta: ch.iter().take(3).map(|c| c.path.rsplit('/').next().unwrap_or(&c.path).to_string()).collect::<Vec<_>>().join(", "),
            prompt: "Review my uncommitted changes in this checkout, point out bugs, and suggest a commit message.".into(),
        });
    }
    out.truncate(4);
    out
}

/// Detects the project's test command from its manifest files.
pub fn test_command(dir: &Path) -> Option<String> {
    let has = |f: &str| dir.join(f).exists();
    if has("Cargo.toml") {
        return Some("cargo test".into());
    }
    if has("package.json") {
        let pkg = std::fs::read_to_string(dir.join("package.json")).ok()?;
        let v: serde_json::Value = serde_json::from_str(&pkg).ok()?;
        if v["scripts"]["test"].is_string() {
            let pm = if has("pnpm-lock.yaml") {
                "pnpm"
            } else if has("yarn.lock") {
                "yarn"
            } else if has("bun.lockb") || has("bun.lock") {
                "bun"
            } else {
                "npm"
            };
            return Some(format!("{pm} test"));
        }
    }
    if has("go.mod") {
        return Some("go test ./...".into());
    }
    if has("pyproject.toml") || has("pytest.ini") || has("setup.cfg") {
        return Some("pytest".into());
    }
    if std::fs::read_dir(dir).ok()?.flatten().any(|e| e.path().extension().is_some_and(|x| x == "sln" || x == "csproj")) {
        return Some("dotnet test".into());
    }
    None
}

/// Runs the test command locally. Returns (passed?, summary). These are local results, not CI.
pub fn run_tests(cwd: &Path, cmd: &str) -> (Option<bool>, String) {
    let mut c = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/C", cmd]);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", cmd]);
        c
    };
    c.current_dir(cwd).stdin(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    match crate::provider::process::output_with_timeout(c, Duration::from_secs(20 * 60)) {
        Ok(o) => {
            let text = format!("{}\n{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
            let ok = o.status.success();
            (
                Some(ok),
                summarize(&text).unwrap_or_else(|| {
                    if ok { format!("{cmd} passed") } else { format!("{cmd} failed (exit {})", o.status.code().unwrap_or(-1)) }
                }),
            )
        }
        Err(e) => (None, format!("{cmd}: {e}")),
    }
}

/// Pulls a pass/fail count out of common test runner output.
pub fn summarize(out: &str) -> Option<String> {
    let (mut passed, mut failed, mut seen) = (0u64, 0u64, false);
    for line in out.lines() {
        let l = line.trim();
        // cargo: "test result: ok. 14 passed; 0 failed; ..."
        if let Some(rest) = l.strip_prefix("test result:") {
            seen = true;
            for part in rest.split(';') {
                let w: Vec<&str> = part.split_whitespace().collect();
                if let Some(i) = w.iter().position(|x| *x == "passed" || *x == "failed") {
                    let n: u64 = w.get(i.wrapping_sub(1)).and_then(|x| x.trim_start_matches(['.', ' ']).parse().ok()).unwrap_or(0);
                    if w[i] == "passed" { passed += n } else { failed += n }
                }
            }
        }
        // pytest: "=== 3 passed, 1 failed in 0.12s ==="
        if l.starts_with('=') && (l.contains(" passed") || l.contains(" failed")) && l.contains(" in ") {
            seen = true;
            for part in l.trim_matches('=').split(',') {
                let w: Vec<&str> = part.split_whitespace().collect();
                if w.len() >= 2 {
                    let n: u64 = w[0].parse().unwrap_or(0);
                    if w[1].starts_with("passed") {
                        passed += n
                    } else if w[1].starts_with("failed") {
                        failed += n
                    }
                }
            }
        }
        // jest/vitest: "Tests:       1 failed, 13 passed, 14 total"
        if let Some(rest) = l.strip_prefix("Tests:") {
            seen = true;
            for part in rest.split(',') {
                let w: Vec<&str> = part.split_whitespace().collect();
                if w.len() >= 2 {
                    let n: u64 = w[0].parse().unwrap_or(0);
                    if w[1] == "passed" {
                        passed += n
                    } else if w[1] == "failed" {
                        failed += n
                    }
                }
            }
        }
    }
    seen.then(|| format!("{}/{} passed", passed, passed + failed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summaries_from_real_runner_output() {
        let cargo =
            "running 3 tests\ntest result: ok. 3 passed; 0 failed; 0 ignored\n\ntest result: FAILED. 10 passed; 1 failed; 0 ignored";
        assert_eq!(summarize(cargo).as_deref(), Some("13/14 passed"));
        assert_eq!(summarize("===== 3 passed, 1 failed in 0.12s =====").as_deref(), Some("3/4 passed"));
        assert_eq!(summarize("Tests:       1 failed, 13 passed, 14 total").as_deref(), Some("13/14 passed"));
        assert_eq!(summarize("nothing here"), None);
    }

    #[test]
    fn detects_test_commands() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(test_command(d.path()), None);
        std::fs::write(d.path().join("package.json"), r#"{"scripts":{"test":"vitest"}}"#).unwrap();
        std::fs::write(d.path().join("pnpm-lock.yaml"), "").unwrap();
        assert_eq!(test_command(d.path()).as_deref(), Some("pnpm test"));
        std::fs::write(d.path().join("Cargo.toml"), "").unwrap();
        assert_eq!(test_command(d.path()).as_deref(), Some("cargo test"));
    }
}
