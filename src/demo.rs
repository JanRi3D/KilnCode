//! Explicitly labeled demo mode (`kiln-code --demo`) for visual testing. It uses a temporary
//! data directory and throwaway git repositories; it never touches the user's data and never
//! runs agents. Normal mode never falls back to anything in here.

use crate::app::{App, RightTab, View};
use crate::model::*;
use crate::provider::{ApprovalKind, ApprovalRequest};
use crate::store::Store;
use std::path::{Path, PathBuf};
use std::process::Command;

fn git(dir: &Path, args: &[&str]) {
    let mut c = Command::new("git");
    c.current_dir(dir).args(["-c", "user.name=Kiln Demo", "-c", "user.email=demo@kiln.invalid", "-c", "commit.gpgsign=false"]).args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    let _ = c.output();
}

fn write(dir: &Path, rel: &str, text: &str) {
    let p = dir.join(rel);
    let _ = std::fs::create_dir_all(p.parent().unwrap_or(dir));
    let _ = std::fs::write(p, text);
}

fn repo(root: &Path, name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = root.join(name);
    let _ = std::fs::create_dir_all(&dir);
    git(&dir, &["init", "-q", "-b", "main"]);
    for (p, t) in files {
        write(&dir, p, t);
    }
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-q", "-m", "Initial commit"]);
    dir
}

const NPCAP_BEFORE: &str = "use crate::capture::{CaptureConfig, Result};\nuse crate::bpf::{compile_filter, Linktype};\nuse pcap_sys::*;\n\npub struct NpcapBackend {\n    handle: Option<Handle>,\n}\n\nimpl NpcapBackend {\n    pub fn open(&mut self, cfg: &CaptureConfig) -> Result<()> {\n        let prog = compile_filter(&cfg.filter, Linktype::ETHERNET)?;\n        let handle = pcap_open(&cfg.device)?;\n        handle.set_filter(&prog)?;\n        self.handle = Some(handle);\n        Ok(())\n    }\n}\n";
const NPCAP_AFTER: &str = "use crate::capture::{CaptureConfig, Result};\nuse pcap_sys::*;\n\npub struct NpcapBackend {\n    handle: Option<Handle>,\n}\n\nimpl NpcapBackend {\n    pub fn open(&mut self, cfg: &CaptureConfig) -> Result<()> {\n        // Compile against the opened handle so the link type matches the adapter.\n        let handle = pcap_open(&cfg.device)?;\n        let prog = handle.compile(&cfg.filter, true)?;\n        handle.set_filter(&prog)?;\n        log::debug!(\"filter compiled for {:?}\", handle.linktype());\n        self.handle = Some(handle);\n        Ok(())\n    }\n}\n";
const MOD_BEFORE: &str = "pub mod npcap;\n\npub fn start(cfg: CaptureConfig) -> Result<Session> {\n    let mut backend = Backend::for_platform();\n    backend.open(&cfg).context(\"open capture\")?;\n    Ok(Session::new(backend))\n}\n";
const MOD_AFTER: &str = "pub mod npcap;\n\npub fn start(cfg: CaptureConfig) -> Result<Session> {\n    let mut backend = Backend::for_platform();\n    backend.open(&cfg)\n        .with_context(|| format!(\"open capture on {}\", cfg.device))?;\n    Ok(Session::new(backend))\n}\n";
const TEST: &str = "#![cfg(windows)]\nuse netmole::capture::{start, CaptureConfig};\n\n#[test]\nfn loopback_tcp_443_matches() {\n    let cfg = CaptureConfig::loopback().filter(\"tcp port 443\");\n    let session = start(cfg).expect(\"open loopback adapter\");\n    let pkt = session.inject_test_packet(443).unwrap();\n    assert!(session.matched(&pkt));\n}\n";

fn item(store: &Store, tid: &str, seq: i64, kind: ItemKind) {
    store.save_item(&Item { id: new_id(), thread_id: tid.to_string(), seq, created_at: now() - 600 + seq, kind });
}

fn tool(kind: ToolKind, label: &str) -> ItemKind {
    ItemKind::Tool { tool_id: new_id(), kind, label: label.into(), detail: None, output: None, status: ToolStatus::Ok, paths: vec![] }
}

pub fn seed(store: &Store) {
    if !store.load_threads().is_empty() {
        return;
    }
    let root = crate::log::data_dir().join("demo-repos");
    let netmole = repo(
        &root,
        "netmole",
        &[
            ("Cargo.toml", "[package]\nname = \"netmole\"\nversion = \"0.4.2\"\n"),
            ("src/capture/npcap.rs", NPCAP_BEFORE),
            ("src/capture/mod.rs", MOD_BEFORE),
            ("src/filter.rs", "// TODO: handle IPv6 extension headers when matching ports\npub fn matches() {}\n"),
        ],
    );
    let base = String::from_utf8_lossy(
        &Command::new("git").current_dir(&netmole).args(["rev-parse", "HEAD"]).output().map(|o| o.stdout).unwrap_or_default(),
    )
    .trim()
    .to_string();
    git(&netmole, &["checkout", "-q", "-b", "kiln/fix-bpf-filter-windows"]);
    write(&netmole, "src/capture/npcap.rs", NPCAP_AFTER);
    write(&netmole, "src/capture/mod.rs", MOD_AFTER);
    write(&netmole, "tests/filter_windows.rs", TEST);
    let bonfire = repo(
        &root,
        "Bonfire",
        &[("README.md", "# Bonfire\nA game launcher.\n"), ("src/library/scan.rs", "// TODO: skip broken symlinks during scan\n")],
    );
    let homelab = repo(&root, "homelab", &[("docker-compose.yml", "services:\n  grafana:\n    image: grafana/grafana\n")]);

    let mk = |name: &str, path: &Path| Project {
        id: new_id(),
        name: name.into(),
        path: path.to_path_buf(),
        pinned: false,
        archived: false,
        created_at: now() - 86_400 * 9,
        last_used_at: now(),
        scripts: vec![("start".into(), "cargo run".into())],
    };
    let pn = mk("netmole", &netmole);
    let pb = mk("Bonfire", &bonfire);
    let ph = mk("homelab", &homelab);
    for p in [&pn, &pb, &ph] {
        store.save_project(p);
    }
    let thread = |p: &Project, title: &str, provider: ProviderKind, model: &str, state: RunState, ago: i64| {
        let mut t = Thread::new(Some(p.id.clone()), title.into(), provider, model.into());
        t.state = state;
        t.cwd = Some(p.path.clone());
        t.branch = Some(format!("kiln/{}", crate::git::slugify(title, 40)));
        t.updated_at = now() - ago;
        t.created_at = now() - ago - 600;
        t.effort = Some("high".into());
        t
    };
    let mut t1 = thread(&pn, "Fix BPF capture filter on Windows", ProviderKind::Claude, "opus", RunState::Completed, 120);
    t1.base_rev = Some(base.clone());
    t1.is_worktree = true;
    t1.worked_secs = 112;
    t1.usage = Usage {
        context_tokens: Some(76_000),
        context_window: Some(200_000),
        total_input: 120_000,
        total_output: 6_000,
        cached_input: 90_000,
        est_cost_usd: Some(0.42),
    };
    t1.plan = vec![
        PlanStep { step: "Find why Windows filters never match".into(), status: StepStatus::Completed },
        PlanStep { step: "Compile against the opened adapter".into(), status: StepStatus::Completed },
        PlanStep { step: "Add a loopback regression test".into(), status: StepStatus::Completed },
        PlanStep { step: "Run the capture tests".into(), status: StepStatus::InProgress },
    ];
    store.save_thread(&t1);
    let mut s = 0;
    let mut add = |k: ItemKind| {
        item(store, &t1.id, s, k);
        s += 1;
    };
    add(ItemKind::User {
        text: "Capture filters like `tcp port 443` work on Linux but match nothing on Windows. Find out why and fix it.".into(),
        attachments: vec![],
    });
    for (k, l) in [
        (ToolKind::Read, "Read src/capture/mod.rs"),
        (ToolKind::Search, "Searched for \"compile_filter\" in src/"),
        (ToolKind::Read, "Read src/capture/npcap.rs"),
        (ToolKind::Edit, "Edited src/capture/npcap.rs"),
        (ToolKind::Edit, "Edited src/capture/mod.rs"),
        (ToolKind::Edit, "Wrote tests/filter_windows.rs"),
    ] {
        add(tool(k, l));
    }
    add(ItemKind::Decision { label: "Ran cargo check · allowed by your rule “cargo check *”".into(), allowed: true });
    add(ItemKind::Assistant { text: "Found it. On Windows the filter is compiled **before** the adapter is opened, so it is built for Ethernet framing. The Npcap loopback adapter uses `DLT_NULL`, which shifts every offset, so nothing ever matches. Linux opens the adapter first, which is why it works there.\n\nI moved the compile step after `pcap_open` so it uses the adapter's own link type, and added a Windows regression test:\n\n```rust\nlet handle = pcap_open(&cfg.device)?;\nlet prog = handle.compile(&cfg.filter, true)?;\n```\n\nI'd like to run the capture tests to confirm.".into() });
    add(ItemKind::TurnEnd { secs: 112, files: 3, additions: 24, deletions: 4 });

    let mut t2 = thread(&pn, "Add pcap export", ProviderKind::Codex, "", RunState::Completed, 300);
    t2.additions = 61;
    t2.deletions = 3;
    t2.plan = vec![
        PlanStep { step: "Read the capture loop".into(), status: StepStatus::Completed },
        PlanStep { step: "Add the --export flag".into(), status: StepStatus::Completed },
        PlanStep { step: "Write packets with pcap-file".into(), status: StepStatus::InProgress },
        PlanStep { step: "Test the export".into(), status: StepStatus::Pending },
    ];
    store.save_thread(&t2);
    item(
        store,
        &t2.id,
        0,
        ItemKind::User { text: "Add an `--export out.pcap` flag that writes captured packets to a pcap file.".into(), attachments: vec![] },
    );
    item(store, &t2.id, 1, ItemKind::Assistant { text: "Writing the export path in `src/export.rs`. The `pcap-file` crate is already in the lockfile, so no new dependency is needed.".into() });
    let mut t_refactor = thread(&pn, "Refactor packet parser", ProviderKind::Claude, "sonnet", RunState::ReadyToReview, 3600);
    t_refactor.additions = 340;
    t_refactor.deletions = 410;
    t_refactor.files_changed = 6;
    t_refactor.worked_secs = 400;
    store.save_thread(&t_refactor);
    item(
        store,
        &t_refactor.id,
        0,
        ItemKind::User {
            text: "Split the packet parser into one module per protocol. Keep the public API unchanged.".into(),
            attachments: vec![],
        },
    );
    item(
        store,
        &t_refactor.id,
        1,
        ItemKind::Assistant { text: "Done. `parser.rs` is now `parser/{eth,ip,tcp,udp}.rs` and the public API is identical.".into() },
    );
    item(store, &t_refactor.id, 2, ItemKind::TurnEnd { secs: 400, files: 6, additions: 340, deletions: 410 });
    let mut t4 = thread(&pb, "Steam library import", ProviderKind::Claude, "sonnet", RunState::Completed, 500);
    t4.additions = 112;
    t4.deletions = 8;
    store.save_thread(&t4);
    item(
        store,
        &t4.id,
        0,
        ItemKind::User {
            text: "Import the Steam library by reading `libraryfolders.vdf` and the app manifests.".into(),
            attachments: vec![],
        },
    );
    let mut t5 = thread(&ph, "Grafana alert rules", ProviderKind::Codex, "", RunState::Completed, 86_400 * 2);
    t5.settled = true;
    t5.additions = 88;
    store.save_thread(&t5);
    let mut t6 = thread(&pb, "Fix controller hotplug", ProviderKind::Claude, "opus", RunState::Completed, 720);
    t6.additions = 31;
    t6.deletions = 6;
    store.save_thread(&t6);
    item(
        store,
        &t6.id,
        0,
        ItemKind::User { text: "Controllers plugged in after launch are not detected. Fix hotplug.".into(), attachments: vec![] },
    );
    item(store, &t6.id, 1, ItemKind::Assistant { text: "The hotplug listener now re-scans `/dev/input` on udev add events. The fix also needs a device mapping in `~/.config/bonfire/input.toml`, which is outside the worktree.".into() });
    // A sample comparison (demo data only).
    let group = new_id();
    for (i, (prov, model, tests, ok, a, d)) in [
        (ProviderKind::Claude, "opus", "14/14 passed", true, 24, 4),
        (ProviderKind::Codex, "", "13/14 passed", false, 41, 12),
        (ProviderKind::Claude, "sonnet", "14/14 passed", true, 52, 9),
    ]
    .into_iter()
    .enumerate()
    {
        let mut t = thread(&pn, "Fix BPF capture filter on Windows", prov, model, RunState::ReadyToReview, 180);
        t.compare_group = Some(group.clone());
        t.run_label = Some(((b'A' + i as u8) as char).to_string());
        t.test_summary = Some(tests.into());
        t.test_ok = Some(ok);
        t.additions = a;
        t.deletions = d;
        t.files_changed = 3;
        t.worked_secs = 110 + i as i64 * 40;
        t.base_rev = Some(base.clone());
        t.base_branch = Some("main".into());
        store.save_thread(&t);
        item(
            store,
            &t.id,
            0,
            ItemKind::User {
                text: "Capture filters like `tcp port 443` work on Linux but match nothing on Windows. Find out why and fix it.".into(),
                attachments: vec![],
            },
        );
        item(
            store,
            &t.id,
            1,
            ItemKind::Assistant {
                text: [
                    "Compiles the filter against the opened handle, so the link type always matches, and adds a loopback regression test.",
                    "Hard-codes DLT_NULL for loopback adapters. ethernet_filter_unchanged fails because the Ethernet path changed too.",
                    "Found the same cause as run A and also rewrote the filter cache.",
                ][i]
                    .into(),
            },
        );
    }
    store.save_rule(&ApprovalRule {
        id: new_id(),
        project_id: Some(pn.id.clone()),
        pattern: "cargo check *".into(),
        decision: RuleDecision::Allow,
        created_at: now(),
        uses: 41,
        builtin: false,
    });
    store.flush();
}

/// Runtime-only demo state: a pending approval card and a "working" thread (no process runs).
pub fn attach_runtime(app: &mut App) {
    let find = |app: &App, title: &str| app.threads.iter().find(|t| t.title == title && t.compare_group.is_none()).map(|t| t.id.clone());
    if let Some(id) = find(app, "Fix BPF capture filter on Windows") {
        if let Some(t) = app.thread_mut(&id) {
            t.state = RunState::NeedsApproval;
        }
        let cwd = app.thread(&id).and_then(|t| t.cwd.clone()).map(|c| c.display().to_string());
        app.approvals.insert(
            id.clone(),
            vec![ApprovalRequest {
                request_id: "demo".into(),
                kind: ApprovalKind::Command,
                title: "Run this command?".into(),
                tool_name: Some("Bash".into()),
                command: Some("cargo test --features npcap capture::".into()),
                cwd,
                paths: vec![],
                reason: Some("Builds and runs the capture tests to confirm the fix.".into()),
                preview: None,
                read_only: false,
                network: false,
                session_option: Some("Always allow Bash(cargo test:*) this session".into()),
            }],
        );
        app.current = Some(id.clone());
        app.open_thread(&id);
    }
    if let Some(id) = find(app, "Add pcap export")
        && let Some(t) = app.thread_mut(&id)
    {
        t.state = RunState::Working;
        t.turn_started_at = Some(now() - 312);
        t.last_activity = Some("Writing src/export.rs".into());
    }
    if let Some(id) = find(app, "Steam library import")
        && let Some(t) = app.thread_mut(&id)
    {
        t.state = RunState::Working;
        t.turn_started_at = Some(now() - 511);
        t.last_activity = Some("Mapping app manifests".into());
    }
}

/// `KILN_VIEW` for screenshots: thread, new, board, compare, review, palette, terminal, diff, timeline, settings:<page>.
pub fn apply_view(app: &mut App, v: &str) {
    match v {
        "new" => app.view = View::NewThread,
        "board" => app.view = View::Board,
        "review" => app.view = View::Review,
        "palette" => app.ui.palette_open = true,
        "terminal" => app.ui.terminal_open = true,
        "diff" => {
            app.ui.right_open = true;
            app.ui.right_tab = Some(RightTab::Diff);
        }
        "timeline" => {
            app.ui.right_open = true;
            app.ui.right_tab = Some(RightTab::Timeline);
        }
        "compare" => {
            if let Some(g) = app.threads.iter().find_map(|t| t.compare_group.clone()) {
                app.view = View::Compare(g);
            }
        }
        s if s.starts_with("settings:") => {
            app.ui.settings_open = true;
            app.ui.settings_page = s["settings:".len()..].to_string();
        }
        _ => {}
    }
}
