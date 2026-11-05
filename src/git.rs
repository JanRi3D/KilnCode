//! Git and GitHub CLI integration for Kiln Code.
//!
//! Everything shells out to the user's installed `git` and `gh` executables (no libgit2).
//! All functions block; call them from background threads.
//!
//! Invariants:
//! - Every git call runs with `core.quotepath=false`, `color.ui=never`, `GIT_TERMINAL_PROMPT=0`,
//!   stdin closed (or fed by us) and, on Windows, `CREATE_NO_WINDOW`.
//! - Inherited `GIT_DIR` / `GIT_WORK_TREE` / `GIT_INDEX_FILE` are removed so a parent process
//!   (e.g. a git hook that launched the app) cannot redirect us to another repository.
//! - Nothing here destroys user work: no `reset --hard`, no `clean`, no forced removals.
//!   The real index is never written except by `commit_all` (`git add -A`).
//! - Paths in results are repository-root relative, `/`-separated, UTF-8 (lossy for non-UTF-8 names).

#![allow(dead_code)] // The app adopts this API incrementally.

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::io::{Read, Write};
use std::iter::Peekable;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitError {
    pub message: String,
    pub stderr: String,
}

impl GitError {
    fn new(message: impl Into<String>) -> Self {
        Self { message: message.into(), stderr: String::new() }
    }
}

impl fmt::Display for GitError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.stderr.trim() {
            "" => f.write_str(&self.message),
            stderr => write!(f, "{}: {}", self.message, stderr),
        }
    }
}

impl std::error::Error for GitError {}

pub type Result<T> = std::result::Result<T, GitError>;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct BranchInfo {
    /// `main` for local branches, `origin/main` for remote-tracking branches.
    pub name: String,
    pub is_remote: bool,
    pub is_current: bool,
    /// Short upstream name, e.g. `origin/main`.
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
    Copied,
    TypeChanged,
    Untracked,
    Conflicted,
}

#[derive(Clone, Debug)]
pub struct FileChange {
    pub path: String,
    pub old_path: Option<String>,
    pub kind: ChangeKind,
    pub additions: u32,
    pub deletions: u32,
    pub binary: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Add,
    Del,
}

#[derive(Clone, Debug)]
pub struct DiffLine {
    pub kind: LineKind,
    pub old_no: Option<u32>,
    pub new_no: Option<u32>,
    /// Line content without the `+`/`-`/` ` marker and without the trailing `\n` / `\r\n`.
    pub text: String,
}

#[derive(Clone, Debug)]
pub struct Hunk {
    /// The full `@@ -a,b +c,d @@ heading` line.
    pub header: String,
    pub old_start: u32,
    pub old_len: u32,
    pub new_start: u32,
    pub new_len: u32,
    pub lines: Vec<DiffLine>,
    /// Exact patch text of the hunk body (everything after the header line), with original
    /// line endings and `\ No newline at end of file` markers. Used by [`file_patch`].
    pub raw: String,
}

#[derive(Clone, Debug)]
pub struct FileDiff {
    pub path: String,
    pub old_path: Option<String>,
    pub kind: ChangeKind,
    pub binary: bool,
    pub additions: u32,
    pub deletions: u32,
    pub hunks: Vec<Hunk>,
}

#[derive(Clone, Debug)]
pub struct WorktreeInfo {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub head: Option<String>,
    pub is_main: bool,
    pub locked: bool,
    pub prunable: bool,
}

#[derive(Clone, Debug)]
pub struct CommitInfo {
    pub sha: String,
    pub short: String,
    pub subject: String,
    pub author: String,
    pub time_unix: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GhAuth {
    NotInstalled,
    /// Not signed in (or the token is broken); the string is gh's explanation.
    SignedOut(String),
    SignedIn {
        user: String,
        host: String,
    },
}

// ---------------------------------------------------------------------------
// Process plumbing
// ---------------------------------------------------------------------------

fn base_command(program: &str, cwd: Option<&Path>) -> Command {
    let mut cmd = Command::new(program);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    cmd.env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

fn git_cmd<I, S>(cwd: &Path, args: I) -> Command
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut cmd = base_command("git", Some(cwd));
    cmd.args(["-c", "core.quotepath=false", "-c", "color.ui=never"]).args(args);
    cmd
}

fn gh_cmd<I, S>(cwd: Option<&Path>, args: I) -> Command
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut cmd = base_command("gh", cwd);
    cmd.env("GH_PROMPT_DISABLED", "1").env("GH_NO_UPDATE_NOTIFIER", "1").env("GH_SPINNER_DISABLED", "1").env("NO_COLOR", "1").args(args);
    cmd
}

/// `git status --porcelain=v2` style short description for error messages.
fn describe(cmd: &Command) -> String {
    let mut parts = vec![cmd.get_program().to_string_lossy().into_owned()];
    let mut args = cmd.get_args();
    while let Some(arg) = args.next() {
        let arg = arg.to_string_lossy();
        if arg == "-c" {
            args.next();
        } else if arg != "--no-optional-locks" && arg != "--literal-pathspecs" {
            parts.push(arg.into_owned());
            if parts.len() == 3 {
                break;
            }
        }
    }
    parts.join(" ")
}

struct Ran {
    what: String,
    out: Output,
}

impl Ran {
    fn error(&self) -> GitError {
        GitError { message: format!("`{}` failed ({})", self.what, self.out.status), stderr: lossy(&self.out.stderr).trim().to_string() }
    }

    fn ok(self) -> Result<Vec<u8>> {
        if self.out.status.success() { Ok(self.out.stdout) } else { Err(self.error()) }
    }
}

/// Spawns `cmd`, optionally feeding `input` on stdin (from a separate thread so large inputs
/// cannot deadlock against a full stdout pipe). Only spawn failures are errors here.
fn exec(mut cmd: Command, input: Option<&[u8]>) -> Result<Ran> {
    let what = describe(&cmd);
    if input.is_some() {
        cmd.stdin(Stdio::piped());
    }
    let mut child = cmd.spawn().map_err(|e| GitError::new(format!("failed to start `{what}`: {e} (is it installed and on PATH?)")))?;
    let stdin = child.stdin.take();
    let out = std::thread::scope(|s| {
        if let (Some(mut pipe), Some(data)) = (stdin, input) {
            // Dropping `pipe` at the end of the closure closes stdin (EOF for the child).
            s.spawn(move || {
                let _ = pipe.write_all(data);
            });
        }
        child.wait_with_output()
    })
    .map_err(|e| GitError::new(format!("failed to run `{what}`: {e}")))?;
    Ok(Ran { what, out })
}

fn run(cmd: Command, input: Option<&[u8]>) -> Result<Vec<u8>> {
    exec(cmd, input)?.ok()
}

fn git(cwd: &Path, args: &[&str]) -> Result<Vec<u8>> {
    run(git_cmd(cwd, args), None)
}

/// Runs git and returns trimmed stdout as a single string.
fn git_line(cwd: &Path, args: &[&str]) -> Result<String> {
    Ok(lossy(&git(cwd, args)?).trim().to_string())
}

fn lossy(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn io_err(what: &str, e: std::io::Error) -> GitError {
    GitError::new(format!("{what}: {e}"))
}

/// Git prints `/`-separated paths; make them native so they compare equal to `Path`s we build.
fn native_path(s: &str) -> PathBuf {
    if cfg!(windows) { PathBuf::from(s.replace('/', "\\")) } else { PathBuf::from(s) }
}

/// A throwaway index file (`GIT_INDEX_FILE`) so we never touch the real index. Deleted on drop.
struct TempIndex(PathBuf);

impl TempIndex {
    /// `seed_from_real`: start from a copy of the repo's real index (keeps its stat cache, so
    /// `git add -A` only re-hashes files that actually changed). Otherwise start empty.
    fn new(root: &Path, seed_from_real: bool) -> Result<Self> {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!("kiln-index-{}-{}", std::process::id(), COUNTER.fetch_add(1, Ordering::Relaxed)));
        let index = TempIndex(path);
        if seed_from_real {
            let real = root.join(native_path(&git_line(root, &["rev-parse", "--git-path", "index"])?));
            if real.exists() {
                fs::copy(&real, &index.0).map_err(|e| io_err("copying index", e))?;
            }
        }
        Ok(index)
    }

    fn cmd<I, S>(&self, root: &Path, args: I) -> Command
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut cmd = git_cmd(root, args);
        cmd.env("GIT_INDEX_FILE", &self.0);
        cmd
    }
}

impl Drop for TempIndex {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

// ---------------------------------------------------------------------------
// Repository basics
// ---------------------------------------------------------------------------

pub fn git_version() -> Result<String> {
    let mut cmd = base_command("git", None);
    cmd.arg("--version");
    let out = lossy(&run(cmd, None)?);
    let out = out.trim();
    Ok(out.strip_prefix("git version ").unwrap_or(out).to_string())
}

/// Top level of the repository or worktree containing `path` (a file or directory).
pub fn repo_root(path: &Path) -> Result<PathBuf> {
    let dir = if path.is_file() { path.parent().unwrap_or(path) } else { path };
    if !dir.is_dir() {
        return Err(GitError::new(format!("{} is not an existing directory", path.display())));
    }
    let top = git_line(dir, &["rev-parse", "--show-toplevel"])?;
    if top.is_empty() {
        return Err(GitError::new(format!("{} is not inside a git work tree", path.display())));
    }
    Ok(native_path(&top))
}

/// `None` when HEAD is detached.
pub fn current_branch(cwd: &Path) -> Result<Option<String>> {
    let ran = exec(git_cmd(cwd, ["symbolic-ref", "--quiet", "HEAD"]), None)?;
    match ran.out.status.code() {
        Some(0) => {
            let full = lossy(&ran.out.stdout).trim().to_string();
            Ok(Some(full.strip_prefix("refs/heads/").unwrap_or(&full).to_string()))
        }
        Some(1) => Ok(None),
        _ => Err(ran.error()),
    }
}

/// `None` for an unborn HEAD (no commits yet).
pub fn head_sha(cwd: &Path) -> Result<Option<String>> {
    let ran = exec(git_cmd(cwd, ["rev-parse", "--verify", "--quiet", "HEAD^{commit}"]), None)?;
    match ran.out.status.code() {
        Some(0) => Ok(Some(lossy(&ran.out.stdout).trim().to_string())),
        Some(1) => Ok(None),
        _ => Err(ran.error()),
    }
}

/// Resolves `rev` to an object id (not peeled: an annotated tag yields the tag object).
pub fn rev_parse(cwd: &Path, rev: &str) -> Result<String> {
    git_line(cwd, &["rev-parse", "--verify", "--end-of-options", rev])
}

/// Tree id for a diff base. `"HEAD"` on an unborn branch resolves to the empty tree.
fn resolve_tree(cwd: &Path, rev: &str) -> Result<String> {
    match rev_parse(cwd, &format!("{rev}^{{tree}}")) {
        Err(_) if rev == "HEAD" && head_sha(cwd)?.is_none() => empty_tree(cwd),
        other => other,
    }
}

fn ref_exists(cwd: &Path, full_ref: &str) -> bool {
    exec(git_cmd(cwd, ["show-ref", "--verify", "--quiet", full_ref]), None).is_ok_and(|r| r.out.status.success())
}

fn empty_tree(cwd: &Path) -> Result<String> {
    // Hash of an empty tree in this repo's hash algorithm (SHA-1 or SHA-256); stdin is empty.
    git_line(cwd, &["hash-object", "-t", "tree", "--stdin"])
}

/// origin/HEAD's target (e.g. "main"), else local `main`/`master`, else the current branch,
/// else "main".
pub fn default_base_branch(cwd: &Path) -> String {
    if let Ok(target) = git_line(cwd, &["symbolic-ref", "--quiet", "refs/remotes/origin/HEAD"])
        && let Some(branch) = target.strip_prefix("refs/remotes/origin/")
    {
        return branch.to_string();
    }
    for name in ["main", "master"] {
        if ref_exists(cwd, &format!("refs/heads/{name}")) {
            return name.to_string();
        }
    }
    current_branch(cwd).ok().flatten().unwrap_or_else(|| "main".to_string())
}

/// Local branches followed by remote-tracking branches (symbolic refs like origin/HEAD skipped).
pub fn branches(cwd: &Path) -> Result<Vec<BranchInfo>> {
    let mut cmd = git_cmd(
        cwd,
        [
            "for-each-ref",
            "--format=%(refname)%00%(symref)%00%(HEAD)%00%(upstream)%00%(upstream:track,nobracket)",
            "refs/heads",
            "refs/remotes",
        ],
    );
    cmd.env("LC_ALL", "C"); // "ahead N, behind M" is translated otherwise.
    let out = lossy(&run(cmd, None)?);
    let short = |r: &str| r.strip_prefix("refs/heads/").or_else(|| r.strip_prefix("refs/remotes/")).unwrap_or(r).to_string();
    let mut list = Vec::new();
    for line in out.lines() {
        let f: Vec<&str> = line.split('\0').collect();
        let [refname, symref, head, upstream, track] = f[..] else {
            continue;
        };
        if !symref.is_empty() {
            continue;
        }
        let (mut ahead, mut behind) = (0, 0);
        for part in track.split(", ") {
            if let Some(n) = part.strip_prefix("ahead ") {
                ahead = n.parse().unwrap_or(0);
            } else if let Some(n) = part.strip_prefix("behind ") {
                behind = n.parse().unwrap_or(0);
            }
        }
        list.push(BranchInfo {
            name: short(refname),
            is_remote: refname.starts_with("refs/remotes/"),
            is_current: head == "*",
            upstream: (!upstream.is_empty()).then(|| short(upstream)),
            ahead,
            behind,
        });
    }
    Ok(list)
}

// ---------------------------------------------------------------------------
// Status / changes
// ---------------------------------------------------------------------------

/// Working tree + index vs HEAD (empty tree if unborn), including untracked (non-ignored)
/// files, with renames detected among tracked files.
pub fn status(cwd: &Path) -> Result<Vec<FileChange>> {
    changes_since(cwd, "HEAD")
}

/// `base_rev` vs the working tree, including untracked (non-ignored) files.
/// (`"HEAD"` on an unborn branch means the empty tree, here and in [`diff_worktree`].)
pub fn changes_since(cwd: &Path, base_rev: &str) -> Result<Vec<FileChange>> {
    let root = repo_root(cwd)?;
    let base = resolve_tree(&root, base_rev)?;
    changes_vs(&root, &base)
}

/// Options that keep `git diff` output parseable regardless of user config.
const DIFF_OPTS: [&str; 6] = ["--no-color", "--no-ext-diff", "--no-textconv", "--src-prefix=a/", "--dst-prefix=b/", "-M"];

fn changes_vs(root: &Path, base: &str) -> Result<Vec<FileChange>> {
    let mut args = vec!["--no-optional-locks", "diff", "--raw", "--numstat", "-z"];
    args.extend(DIFF_OPTS);
    args.extend([base, "--"]);
    let mut changes = parse_raw_numstat(&git(root, &args)?);

    let st = git(root, &["--no-optional-locks", "status", "--porcelain=v2", "-z", "--untracked-files=all", "--no-renames"])?;
    let (untracked, conflicted) = parse_status_v2(&st);
    for c in &mut changes {
        if conflicted.contains(&c.path) {
            c.kind = ChangeKind::Conflicted;
        }
    }
    for path in conflicted {
        if !changes.iter().any(|c| c.path == path) {
            changes.push(change(path, None, ChangeKind::Conflicted));
        }
    }
    for path in untracked {
        let (additions, binary) = count_new_file(&root.join(&path));
        changes.push(FileChange { additions, binary, ..change(path, None, ChangeKind::Untracked) });
    }
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    changes.dedup_by(|a, b| a.path == b.path);
    Ok(changes)
}

fn change(path: String, old_path: Option<String>, kind: ChangeKind) -> FileChange {
    FileChange { path, old_path, kind, additions: 0, deletions: 0, binary: false }
}

fn kind_from_letter(letter: char) -> ChangeKind {
    match letter {
        'A' => ChangeKind::Added,
        'D' => ChangeKind::Deleted,
        'R' => ChangeKind::Renamed,
        'C' => ChangeKind::Copied,
        'T' => ChangeKind::TypeChanged,
        'U' => ChangeKind::Conflicted,
        _ => ChangeKind::Modified,
    }
}

/// Parses `git diff --raw --numstat -z` (raw records first, then numstat records).
fn parse_raw_numstat(out: &[u8]) -> Vec<FileChange> {
    let text = lossy(out);
    let mut toks = text.split('\0');
    let mut changes = Vec::new();
    let mut stats: HashMap<&str, (u32, u32, bool)> = HashMap::new();
    while let Some(tok) = toks.next() {
        if tok.is_empty() {
            continue;
        }
        if let Some(meta) = tok.strip_prefix(':') {
            // ":100644 100644 <sha> <sha> R087" then one path, or two for renames/copies.
            let letter = meta.split(' ').nth(4).and_then(|s| s.chars().next()).unwrap_or('M');
            let first = toks.next().unwrap_or_default().to_string();
            let c = if matches!(letter, 'R' | 'C') {
                let new = toks.next().unwrap_or_default().to_string();
                change(new, Some(first), kind_from_letter(letter))
            } else {
                change(first, None, kind_from_letter(letter))
            };
            changes.push(c);
        } else {
            // "adds\tdels\tpath", or "adds\tdels\t" followed by old and new path tokens.
            let mut parts = tok.splitn(3, '\t');
            let adds = parts.next().unwrap_or("-");
            let dels = parts.next().unwrap_or("-");
            let mut path = parts.next().unwrap_or("");
            if path.is_empty() {
                toks.next();
                path = toks.next().unwrap_or("");
            }
            stats.insert(path, (adds.parse().unwrap_or(0), dels.parse().unwrap_or(0), adds == "-"));
        }
    }
    for c in &mut changes {
        if let Some(&(a, d, bin)) = stats.get(c.path.as_str()) {
            (c.additions, c.deletions, c.binary) = (a, d, bin);
        }
    }
    changes
}

/// Returns (untracked paths, conflicted paths) from `git status --porcelain=v2 -z`.
fn parse_status_v2(out: &[u8]) -> (Vec<String>, HashSet<String>) {
    let text = lossy(out);
    let mut toks = text.split('\0');
    let (mut untracked, mut conflicted) = (Vec::new(), HashSet::new());
    while let Some(tok) = toks.next() {
        match tok.as_bytes().first() {
            Some(b'?') => untracked.push(tok[2..].to_string()),
            Some(b'u') => {
                if let Some(p) = tok.splitn(11, ' ').nth(10) {
                    conflicted.insert(p.to_string());
                }
            }
            Some(b'2') => {
                toks.next(); // rename/copy source path
            }
            _ => {}
        }
    }
    (untracked, conflicted)
}

const BINARY_SNIFF: u64 = 8 * 1024;
const MAX_COUNT_BYTES: u64 = 10 * 1024 * 1024;

/// (line count, binary) for an untracked file, the way `git diff --numstat` would count it.
/// Binary = NUL byte in the first 8 KiB. Files over 10 MiB are not counted (0 lines).
fn count_new_file(path: &Path) -> (u32, bool) {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return (0, false);
    };
    if meta.file_type().is_symlink() {
        return (1, false); // git stores the link target as a one-line blob
    }
    if !meta.is_file() {
        return (0, false); // e.g. "nested-repo/" directories
    }
    let Ok(mut file) = fs::File::open(path) else {
        return (0, false);
    };
    let mut data = Vec::new();
    if (&mut file).take(BINARY_SNIFF).read_to_end(&mut data).is_err() {
        return (0, false);
    }
    if data.contains(&0) {
        return (0, true);
    }
    if meta.len() > MAX_COUNT_BYTES || file.read_to_end(&mut data).is_err() {
        return (0, false);
    }
    let mut lines = data.iter().filter(|&&b| b == b'\n').count();
    if data.last().is_some_and(|&b| b != b'\n') {
        lines += 1;
    }
    (u32::try_from(lines).unwrap_or(u32::MAX), false)
}

/// Any tracked change or untracked (non-ignored) file.
pub fn is_dirty(cwd: &Path) -> Result<bool> {
    let out = git(cwd, &["--no-optional-locks", "status", "--porcelain=v2", "-z", "--untracked-files=normal"])?;
    Ok(!out.is_empty())
}

// ---------------------------------------------------------------------------
// Unified diffs
// ---------------------------------------------------------------------------

/// `base_rev` vs the working tree. Untracked (non-ignored) files are included as new files with
/// kind `Untracked`. Uses a temporary copy of the index with intent-to-add entries; the real
/// index is never modified.
pub fn diff_worktree(cwd: &Path, base_rev: &str) -> Result<Vec<FileDiff>> {
    let root = repo_root(cwd)?;
    let base = resolve_tree(&root, base_rev)?;
    let untracked: Vec<String> = lossy(&git(&root, &["ls-files", "--others", "--exclude-standard", "-z"])?)
        .split('\0')
        .filter(|p| !p.is_empty() && !p.ends_with('/')) // skip nested repositories
        .map(str::to_string)
        .collect();

    let index = TempIndex::new(&root, true)?;
    if !untracked.is_empty() {
        let list = untracked.join("\0");
        run(
            index.cmd(&root, ["--literal-pathspecs", "add", "--intent-to-add", "--pathspec-from-file=-", "--pathspec-file-nul"]),
            Some(list.as_bytes()),
        )?;
    }
    let mut args = vec!["--no-optional-locks", "diff"];
    args.extend(DIFF_OPTS);
    args.extend([base.as_str(), "--"]);
    let out = run(index.cmd(&root, &args), None)?;
    let mut diffs = parse_unified_diff(&lossy(&out));
    let untracked: HashSet<String> = untracked.into_iter().collect();
    for d in &mut diffs {
        if d.kind == ChangeKind::Added && untracked.contains(&d.path) {
            d.kind = ChangeKind::Untracked;
        }
    }
    Ok(diffs)
}

pub fn diff_revs(cwd: &Path, from: &str, to: &str) -> Result<Vec<FileDiff>> {
    let root = repo_root(cwd)?;
    let (from, to) = (resolve_tree(&root, from)?, resolve_tree(&root, to)?);
    let mut args = vec!["diff"];
    args.extend(DIFF_OPTS);
    args.extend([from.as_str(), to.as_str(), "--"]);
    Ok(parse_unified_diff(&lossy(&git(&root, &args)?)))
}

/// Parses `git diff` patch output. Pure. Handles new/deleted/renamed/copied files, mode and
/// type changes, binary files, quoted paths, `\ No newline at end of file` and CRLF content.
/// Combined (`diff --cc`) sections become `Conflicted` entries without hunks.
pub fn parse_unified_diff(text: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut lines = text.split_inclusive('\n').peekable();
    let mut combined = false;
    let mut old_mode = String::new();
    while let Some(raw) = lines.next() {
        let line = raw.trim_end_matches(['\n', '\r']);
        let start = match line.strip_prefix("diff --git ") {
            Some(rest) => Some((git_header_path(rest), ChangeKind::Modified)),
            None => line
                .strip_prefix("diff --cc ")
                .or_else(|| line.strip_prefix("diff --combined "))
                .map(|p| (unquote(p), ChangeKind::Conflicted)),
        };
        if let Some((path, kind)) = start {
            combined = kind == ChangeKind::Conflicted;
            old_mode.clear();
            files.push(FileDiff { path, old_path: None, kind, binary: false, additions: 0, deletions: 0, hunks: Vec::new() });
            continue;
        }
        let Some(f) = files.last_mut() else {
            continue; // preamble before the first file header
        };
        if combined {
            continue;
        }
        if line.starts_with("@@ ") {
            if let Some(h) = parse_hunk(line, &mut lines) {
                for l in &h.lines {
                    match l.kind {
                        LineKind::Add => f.additions += 1,
                        LineKind::Del => f.deletions += 1,
                        LineKind::Context => {}
                    }
                }
                f.hunks.push(h);
            }
        } else if line.starts_with("new file mode ") {
            f.kind = ChangeKind::Added;
        } else if line.starts_with("deleted file mode ") {
            f.kind = ChangeKind::Deleted;
        } else if let Some(v) = line.strip_prefix("rename from ") {
            f.kind = ChangeKind::Renamed;
            f.old_path = Some(unquote(v));
        } else if let Some(v) = line.strip_prefix("rename to ") {
            f.path = unquote(v);
        } else if let Some(v) = line.strip_prefix("copy from ") {
            f.kind = ChangeKind::Copied;
            f.old_path = Some(unquote(v));
        } else if let Some(v) = line.strip_prefix("copy to ") {
            f.path = unquote(v);
        } else if let Some(v) = line.strip_prefix("old mode ") {
            old_mode = v.to_string();
        } else if let Some(v) = line.strip_prefix("new mode ") {
            // 100644 -> 100755 is a mode change; 100644 -> 120000 changes the file type.
            let ty = |m: &str| m[..m.len().saturating_sub(3)].to_string();
            if ty(&old_mode) != ty(v) {
                f.kind = ChangeKind::TypeChanged;
            }
        } else if let Some(v) = line.strip_prefix("--- ") {
            if let Some(p) = patch_name(v, "a/") {
                f.path = p;
            }
        } else if let Some(v) = line.strip_prefix("+++ ") {
            if let Some(p) = patch_name(v, "b/") {
                f.path = p;
            }
        } else if line.starts_with("Binary files ") || line == "GIT binary patch" {
            f.binary = true;
        }
    }
    files
}

fn parse_hunk_header(line: &str) -> Option<(u32, u32, u32, u32)> {
    let rest = line.strip_prefix("@@ -")?;
    let ranges = &rest[..rest.find(" @@")?];
    let (old, new) = ranges.split_once(" +")?;
    let range = |s: &str| -> Option<(u32, u32)> {
        match s.split_once(',') {
            Some((start, len)) => Some((start.parse().ok()?, len.parse().ok()?)),
            None => Some((s.parse().ok()?, 1)),
        }
    };
    let ((os, ol), (ns, nl)) = (range(old)?, range(new)?);
    Some((os, ol, ns, nl))
}

/// Consumes the hunk body following `header`, using the header's line counts so that content
/// lines that look like headers (`--- x`, `diff --git`) are never misread.
fn parse_hunk<'a, I>(header: &str, lines: &mut Peekable<I>) -> Option<Hunk>
where
    I: Iterator<Item = &'a str>,
{
    let (old_start, old_len, new_start, new_len) = parse_hunk_header(header)?;
    let mut hunk = Hunk { header: header.to_string(), old_start, old_len, new_start, new_len, lines: Vec::new(), raw: String::new() };
    let (mut old_no, mut new_no) = (old_start, new_start);
    let (mut old_left, mut new_left) = (old_len, new_len);
    while let Some(&raw) = lines.peek() {
        let body = raw.strip_suffix('\n').unwrap_or(raw);
        let body = body.strip_suffix('\r').unwrap_or(body);
        let kind = match body.as_bytes().first() {
            Some(b'\\') => None, // "\ No newline at end of file" for the previous line
            _ if old_left == 0 && new_left == 0 => break,
            Some(b'+') if new_left > 0 => Some(LineKind::Add),
            Some(b'-') if old_left > 0 => Some(LineKind::Del),
            Some(b' ') | None if old_left > 0 && new_left > 0 => Some(LineKind::Context),
            _ => break, // malformed / truncated hunk
        };
        if let Some(kind) = kind {
            let text = body.get(1..).unwrap_or("").to_string();
            let (o, n) = match kind {
                LineKind::Context => (Some(old_no), Some(new_no)),
                LineKind::Del => (Some(old_no), None),
                LineKind::Add => (None, Some(new_no)),
            };
            if o.is_some() {
                old_no += 1;
                old_left -= 1;
            }
            if n.is_some() {
                new_no += 1;
                new_left -= 1;
            }
            hunk.lines.push(DiffLine { kind, old_no: o, new_no: n, text });
        }
        hunk.raw.push_str(raw);
        lines.next();
    }
    Some(hunk)
}

/// New-side path (prefix stripped) from the `a/x b/y` part of a `diff --git` line. For renames
/// the authoritative names come from the following `rename from/to` and `---`/`+++` lines.
fn git_header_path(rest: &str) -> String {
    let strip = |s: &str, prefix: &str| s.strip_prefix(prefix).unwrap_or(s).to_string();
    if rest.starts_with('"') {
        return strip(&unquote(unquote_prefix(rest).1.trim_start()), "b/");
    }
    if let Some((_, b)) = rest.rsplit_once(" \"") {
        return strip(&unquote(&format!("\"{b}")), "b/");
    }
    // Unquoted names may contain spaces: find the split where both sides name the same path.
    for (i, _) in rest.match_indices(' ') {
        let b = strip(&rest[i + 1..], "b/");
        if strip(&rest[..i], "a/") == b {
            return b;
        }
    }
    rest.split_once(" b/").map_or_else(|| rest.to_string(), |(_, b)| b.to_string())
}

/// Name from a `---` / `+++` line; `None` for /dev/null.
fn patch_name(v: &str, prefix: &str) -> Option<String> {
    let name = if v.starts_with('"') {
        unquote(v)
    } else {
        // git appends a tab after names containing spaces; traditional diffs put a date there.
        v.split('\t').next().unwrap_or(v).to_string()
    };
    if name == "/dev/null" {
        return None;
    }
    Some(name.strip_prefix(prefix).map(str::to_string).unwrap_or(name))
}

fn unquote(s: &str) -> String {
    unquote_prefix(s).0
}

/// Decodes a leading C-style quoted string (git's path quoting) and returns the remainder.
/// Unquoted input is returned unchanged.
fn unquote_prefix(s: &str) -> (String, &str) {
    let Some(body) = s.strip_prefix('"') else {
        return (s.to_string(), "");
    };
    let bytes = body.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => return (lossy(&out), &body[i + 1..]),
            b'\\' if i + 1 < bytes.len() => {
                i += 1;
                match bytes[i] {
                    b'n' => out.push(b'\n'),
                    b't' => out.push(b'\t'),
                    b'r' => out.push(b'\r'),
                    b'a' => out.push(0x07),
                    b'b' => out.push(0x08),
                    b'f' => out.push(0x0c),
                    b'v' => out.push(0x0b),
                    d @ b'0'..=b'7' => {
                        let mut v = u32::from(d - b'0');
                        for _ in 0..2 {
                            match bytes.get(i + 1) {
                                Some(&o @ b'0'..=b'7') => {
                                    v = v * 8 + u32::from(o - b'0');
                                    i += 1;
                                }
                                _ => break,
                            }
                        }
                        out.push(v as u8);
                    }
                    other => out.push(other), // \\ and \"
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    (lossy(&out), "")
}

/// Quotes a patch name the way git does when it contains `"`, `\` or control characters.
fn quote_name(name: &str) -> String {
    if !name.chars().any(|c| c == '"' || c == '\\' || c.is_ascii_control()) {
        return name.to_string();
    }
    let mut s = String::from("\"");
    for c in name.chars() {
        match c {
            '"' => s.push_str("\\\""),
            '\\' => s.push_str("\\\\"),
            '\n' => s.push_str("\\n"),
            '\t' => s.push_str("\\t"),
            '\r' => s.push_str("\\r"),
            c if c.is_ascii_control() => s.push_str(&format!("\\{:03o}", c as u32)),
            c => s.push(c),
        }
    }
    s.push('"');
    s
}

/// A standalone unified patch containing exactly one hunk of `diff` (for `git apply [-R]`).
/// Renamed/copied files are addressed by their new path (the hunk applies to its content).
/// Returns an empty string if `hunk_index` is out of range.
pub fn file_patch(diff: &FileDiff, hunk_index: usize) -> String {
    let Some(hunk) = diff.hunks.get(hunk_index) else {
        return String::new();
    };
    let a = quote_name(&format!("a/{}", diff.path));
    let b = quote_name(&format!("b/{}", diff.path));
    // Mirror git: a tab after names with spaces keeps `patch` and `git apply` unambiguous.
    let tab = if diff.path.contains(' ') { "\t" } else { "" };
    let mut patch = format!("diff --git {a} {b}\n");
    let (old, new) = match diff.kind {
        ChangeKind::Added | ChangeKind::Untracked => {
            patch.push_str("new file mode 100644\n");
            ("/dev/null".to_string(), format!("{b}{tab}"))
        }
        ChangeKind::Deleted => {
            patch.push_str("deleted file mode 100644\n");
            (format!("{a}{tab}"), "/dev/null".to_string())
        }
        _ => (format!("{a}{tab}"), format!("{b}{tab}")),
    };
    patch.push_str(&format!("--- {old}\n+++ {new}\n{}\n", hunk.header));
    patch.push_str(&hunk.raw);
    if !patch.ends_with('\n') {
        patch.push('\n');
    }
    patch
}

/// Reverse-applies one hunk to the working tree only (index untouched). Only for modified
/// (or renamed/copied) text files. Checks first, so it either applies fully or not at all.
pub fn revert_hunk(cwd: &Path, diff: &FileDiff, hunk_index: usize) -> Result<()> {
    if diff.binary || !matches!(diff.kind, ChangeKind::Modified | ChangeKind::Renamed | ChangeKind::Copied) {
        return Err(GitError::new(format!(
            "cannot revert a hunk of {} ({:?}{}): only modified text files are supported",
            diff.path,
            diff.kind,
            if diff.binary { ", binary" } else { "" }
        )));
    }
    if hunk_index >= diff.hunks.len() {
        return Err(GitError::new(format!("hunk {hunk_index} out of range for {} ({} hunks)", diff.path, diff.hunks.len())));
    }
    let root = repo_root(cwd)?;
    let patch = file_patch(diff, hunk_index);
    for check in [true, false] {
        let mut args = vec!["apply", "-R", "--whitespace=nowarn"];
        if check {
            args.push("--check");
        }
        args.push("-");
        run(git_cmd(&root, &args), Some(patch.as_bytes()))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Branch naming
// ---------------------------------------------------------------------------

/// Latin letters folded to ASCII by [`slugify`] (after lowercasing).
const SLUG_FOLD: [(&str, &str); 11] = [
    ("àáâãäåāă", "a"),
    ("çćč", "c"),
    ("èéêëēęě", "e"),
    ("ìíîïī", "i"),
    ("ñńň", "n"),
    ("òóôõöøō", "o"),
    ("ùúûüūů", "u"),
    ("ýÿ", "y"),
    ("ß", "ss"),
    ("æ", "ae"),
    ("œ", "oe"),
];

/// "Fix BPF capture filter on Windows!" -> "fix-bpf-capture-filter-on-windows".
/// Common Latin accents are folded ("Größe" -> "grosse"), apostrophes and other non-ASCII
/// letters are dropped, everything else separates words. Cut at a word boundary to at most
/// `max_len` bytes (a single overlong first word is hard-cut). Never empty: falls back to "task".
pub fn slugify(text: &str, max_len: usize) -> String {
    let mut cleaned = String::with_capacity(text.len());
    for c in text.chars().flat_map(char::to_lowercase) {
        if let Some((_, ascii)) = SLUG_FOLD.iter().find(|(from, _)| from.contains(c)) {
            cleaned.push_str(ascii);
        } else if c.is_ascii() {
            cleaned.push(c);
        } else if !c.is_alphanumeric() && c != '’' {
            cleaned.push(' ');
        }
    }
    let cleaned = cleaned.replace('\'', "");
    let mut out = String::new();
    for word in cleaned.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()) {
        let sep = usize::from(!out.is_empty());
        if out.len() + sep + word.len() <= max_len {
            if sep == 1 {
                out.push('-');
            }
            out.push_str(word);
        } else {
            if out.is_empty() {
                out.push_str(&word[..max_len]);
            }
            break;
        }
    }
    if out.is_empty() { "task".to_string() } else { out }
}

/// True if `refs/heads/<name>` or `refs/remotes/origin/<name>` exists.
pub fn branch_exists(cwd: &Path, name: &str) -> bool {
    let (local, remote) = (format!("refs/heads/{name}"), format!("refs/remotes/origin/{name}"));
    git(cwd, &["for-each-ref", "--format=%(refname)", &local, &remote])
        .map(|out| lossy(&out).lines().any(|l| l == local || l == remote))
        .unwrap_or(false)
}

/// `<prefix>/<slug>`, then `<prefix>/<slug>-2`, `-3`, ... until the name collides with no local
/// branch, no remote-tracking branch on any remote, and no existing worktree directory named
/// `<slug>[-N]`. A prefix of "kiln", "kiln/" or "" all produce "kiln/<slug>" (empty defaults to
/// "kiln"); trailing slashes are ignored. An empty slug becomes "task".
pub fn unique_branch_name(cwd: &Path, prefix: &str, slug: &str) -> String {
    // Empty prefix means no prefix (Settings → Worktrees → Branch prefix).
    let prefix = prefix.trim().trim_end_matches('/');
    let full = |leaf: &str| if prefix.is_empty() { leaf.to_string() } else { format!("{prefix}/{leaf}") };
    let slug = if slug.is_empty() { "task" } else { slug };
    let taken: HashSet<String> = git(cwd, &["for-each-ref", "--format=%(refname)", "refs/heads", "refs/remotes"])
        .map(|out| {
            lossy(&out)
                .lines()
                .filter_map(|r| {
                    r.strip_prefix("refs/heads/").map(str::to_string).or_else(|| {
                        let rest = r.strip_prefix("refs/remotes/")?;
                        Some(rest.split_once('/')?.1.to_string())
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let dirs: HashSet<String> =
        worktree_list(cwd).unwrap_or_default().iter().filter_map(|w| Some(w.path.file_name()?.to_string_lossy().to_lowercase())).collect();
    (1..)
        .map(|n| match n {
            1 => slug.to_string(),
            n => format!("{slug}-{n}"),
        })
        .find(|leaf| !taken.contains(&full(leaf)) && !dirs.contains(&leaf.to_lowercase()))
        .map(|leaf| full(&leaf))
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Worktrees
// ---------------------------------------------------------------------------

pub fn worktree_list(repo: &Path) -> Result<Vec<WorktreeInfo>> {
    let out = lossy(&git(repo, &["worktree", "list", "--porcelain", "-z"])?);
    let mut list: Vec<WorktreeInfo> = Vec::new();
    // Records are NUL-terminated lines; an empty line ends a worktree.
    for tok in out.split('\0') {
        let (key, val) = tok.split_once(' ').unwrap_or((tok, ""));
        if key == "worktree" {
            list.push(WorktreeInfo {
                path: native_path(val),
                branch: None,
                head: None,
                is_main: list.is_empty(),
                locked: false,
                prunable: false,
            });
            continue;
        }
        let Some(w) = list.last_mut() else { continue };
        match key {
            "HEAD" if !val.bytes().all(|b| b == b'0') => w.head = Some(val.to_string()),
            "branch" => w.branch = Some(val.strip_prefix("refs/heads/").unwrap_or(val).to_string()),
            "locked" => w.locked = true,
            "prunable" => w.prunable = true,
            _ => {}
        }
    }
    Ok(list)
}

/// `git worktree add --no-track -b <new_branch> <path> <base_rev>`. Creates parent directories.
/// Fails without touching anything if `path` exists and is not an empty directory.
/// `--no-track` keeps an agent's plain `git push`/`git pull` from targeting the base branch.
pub fn worktree_add(repo: &Path, path: &Path, new_branch: &str, base_rev: &str) -> Result<()> {
    if new_branch.starts_with('-') || base_rev.starts_with('-') {
        return Err(GitError::new("branch and base revision must not start with '-'"));
    }
    let path = if path.is_absolute() { path.to_path_buf() } else { repo.join(path) };
    if path.exists() && !fs::read_dir(&path).is_ok_and(|mut d| d.next().is_none()) {
        return Err(GitError::new(format!("{} already exists and is not an empty directory", path.display())));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| io_err("creating worktree parent", e))?;
    }
    let mut cmd = git_cmd(repo, ["worktree", "add", "--no-track", "-b", new_branch]);
    cmd.arg(&path).arg(base_rev);
    run(cmd, None).map(drop)
}

/// Removes a clean worktree with plain `git worktree remove` (its branch is kept).
/// Refuses if the worktree has tracked changes or untracked non-ignored files.
/// Ignored files inside it (build output etc.) are deleted along with the directory, as git does.
pub fn worktree_remove(repo: &Path, path: &Path) -> Result<()> {
    if path.exists() && is_dirty(path)? {
        return Err(GitError::new(format!("worktree {} has uncommitted or untracked changes; refusing to remove it", path.display())));
    }
    let mut cmd = git_cmd(repo, ["worktree", "remove"]);
    cmd.arg(path);
    run(cmd, None).map(drop)
}

// ---------------------------------------------------------------------------
// Commit / push / log
// ---------------------------------------------------------------------------

/// `git add -A` + `git commit -F -`. Returns the new commit sha; Err if there is nothing to
/// commit. Runs the user's hooks and signing config as a normal commit would.
pub fn commit_all(cwd: &Path, message: &str) -> Result<String> {
    let root = repo_root(cwd)?;
    git(&root, &["add", "-A"])?;
    let staged = exec(git_cmd(&root, ["diff", "--cached", "--quiet"]), None)?;
    match staged.out.status.code() {
        Some(1) => {}
        Some(0) => return Err(GitError::new("nothing to commit")),
        _ => return Err(staged.error()),
    }
    run(git_cmd(&root, ["commit", "--quiet", "-F", "-"]), Some(message.as_bytes()))?;
    git_line(&root, &["rev-parse", "HEAD"])
}

/// `git push -u origin <branch>`; returns git's combined output (it reports progress on stderr).
pub fn push(cwd: &Path, branch: &str) -> Result<String> {
    if branch.starts_with('-') {
        return Err(GitError::new("branch must not start with '-'"));
    }
    let ran = exec(git_cmd(cwd, ["push", "-u", "origin", branch]), None)?;
    if !ran.out.status.success() {
        return Err(ran.error());
    }
    let combined = format!("{}{}", lossy(&ran.out.stdout), lossy(&ran.out.stderr));
    Ok(combined.trim().to_string())
}

/// Newest first. Empty for an unborn HEAD.
pub fn recent_log(cwd: &Path, n: usize) -> Result<Vec<CommitInfo>> {
    if head_sha(cwd)?.is_none() {
        return Ok(Vec::new());
    }
    let count = format!("-n{n}");
    let out = lossy(&git(cwd, &["log", "-z", &count, "--no-color", "--no-show-signature", "--format=%H%x00%h%x00%s%x00%an%x00%ct"])?);
    let mut fields: Vec<&str> = out.split('\0').collect();
    if fields.last() == Some(&"") {
        fields.pop();
    }
    Ok(fields
        .as_chunks::<5>()
        .0
        .iter()
        .map(|[sha, short, subject, author, time]| CommitInfo {
            sha: sha.trim_start_matches('\n').to_string(),
            short: short.to_string(),
            subject: subject.to_string(),
            author: author.to_string(),
            time_unix: time.trim().parse().unwrap_or(0),
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Checkpoints (refs/kiln/checkpoints/*)
// ---------------------------------------------------------------------------
//
// A checkpoint is a commit whose tree is the working tree's tracked + untracked files, as
// `git add -A` sees them. Ignored files are NOT captured (and never touched by a restore).
// Submodules are recorded as gitlinks only; their contents are not captured or restored.
// The refs namespace is shared by all worktrees of a repository, so `ref_name` must be unique
// across worktrees (e.g. include a session id). Creating a checkpoint writes blobs for changed
// and untracked files into the object database (that is what keeps them restorable).

const CHECKPOINT_NS: &str = "refs/kiln/checkpoints/";

/// Tree of the current working tree (tracked + untracked, ignored excluded), built in a
/// temporary index seeded from the real one (same result as `read-tree HEAD` + `add -A`, but
/// only changed files get re-hashed).
fn snapshot_tree(root: &Path) -> Result<String> {
    let index = TempIndex::new(root, true)?;
    run(index.cmd(root, ["add", "-A"]), None)?;
    Ok(lossy(&run(index.cmd(root, ["write-tree"]), None)?).trim().to_string())
}

/// Snapshots the working tree into `refs/kiln/checkpoints/<ref_name>` without touching HEAD,
/// the real index or any file. Returns the checkpoint commit sha (parent: HEAD, if any).
pub fn checkpoint_create(cwd: &Path, ref_name: &str, message: &str) -> Result<String> {
    let root = repo_root(cwd)?;
    let tree = snapshot_tree(&root)?;
    let mut cmd = git_cmd(&root, ["commit-tree", tree.as_str()]);
    if let Some(head) = head_sha(&root)? {
        cmd.arg("-p").arg(head);
    }
    cmd.args(["-F", "-"]);
    // Fixed identity: checkpoints are private snapshots and must work without user.name config.
    for (k, v) in [
        ("GIT_AUTHOR_NAME", "Kiln Code"),
        ("GIT_AUTHOR_EMAIL", "kiln@localhost"),
        ("GIT_COMMITTER_NAME", "Kiln Code"),
        ("GIT_COMMITTER_EMAIL", "kiln@localhost"),
    ] {
        cmd.env(k, v);
    }
    let sha = lossy(&run(cmd, Some(message.as_bytes()))?).trim().to_string();
    git(&root, &["update-ref", &format!("{CHECKPOINT_NS}{ref_name}"), &sha])?;
    Ok(sha)
}

/// What `checkpoint_restore(sha)` would change, relative to the current working tree:
/// `Added` = file would be recreated, `Deleted` = file would be removed, etc.
pub fn checkpoint_changes(cwd: &Path, sha: &str) -> Result<Vec<FileChange>> {
    let root = repo_root(cwd)?;
    let target = resolve_tree(&root, sha)?;
    let current = snapshot_tree(&root)?;
    let out = git(&root, &["diff-tree", "-r", "-M", "--raw", "--numstat", "-z", "--no-ext-diff", "--no-textconv", &current, &target])?;
    let mut changes = parse_raw_numstat(&out);
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(changes)
}

/// Makes the working tree's tracked + untracked (non-ignored) files match checkpoint `sha`.
/// First snapshots the current state into `refs/kiln/checkpoints/<safety_ref>` and returns that
/// sha (restore it to undo). Only files that differ are written or removed; empty directories
/// left behind are removed. HEAD, the real index and ignored files are not touched.
pub fn checkpoint_restore(cwd: &Path, sha: &str, safety_ref: &str) -> Result<String> {
    let root = repo_root(cwd)?;
    let target = resolve_tree(&root, sha)?;
    let safety = checkpoint_create(&root, safety_ref, &format!("kiln: safety checkpoint before restoring {sha}"))?;
    let current = resolve_tree(&root, &safety)?;
    let out = lossy(&git(&root, &["diff-tree", "-r", "--no-renames", "--raw", "-z", &current, &target])?);

    let (mut remove, mut write) = (Vec::new(), Vec::new());
    let mut toks = out.split('\0');
    while let Some(meta) = toks.next() {
        let Some(meta) = meta.strip_prefix(':') else {
            continue;
        };
        let Some(path) = toks.next() else { break };
        // ":<old mode> <new mode> <old sha> <new sha> <status>"; 160000 = submodule (skipped).
        let f: Vec<&str> = meta.split(' ').collect();
        let [old_mode, new_mode, _, _, status] = f[..] else {
            continue;
        };
        if status.starts_with('D') {
            if old_mode != "160000" {
                remove.push(path);
            }
        } else if new_mode != "160000" {
            write.push(path);
        }
    }

    for path in &remove {
        let full = root.join(path);
        match fs::remove_file(&full) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(io_err(&format!("removing {path} (safety checkpoint {safety})"), e));
            }
        }
        let mut dir = full.parent();
        while let Some(d) = dir {
            if d == root || fs::remove_dir(d).is_err() {
                break; // stops at the first non-empty directory
            }
            dir = d.parent();
        }
    }

    if !write.is_empty() {
        if let Some(p) = write.iter().find(|p| root.join(p).is_dir()) {
            return Err(GitError::new(format!(
                "cannot restore {p}: a directory with untracked or ignored files is in the way \
                 (safety checkpoint {safety})"
            )));
        }
        let index = TempIndex::new(&root, false)?;
        run(index.cmd(&root, ["read-tree", target.as_str()]), None)?;
        let list = write.join("\0");
        run(index.cmd(&root, ["checkout-index", "-f", "-z", "--stdin"]), Some(list.as_bytes()))?;
    }
    Ok(safety)
}

pub fn checkpoint_delete(cwd: &Path, ref_name: &str) -> Result<()> {
    git(cwd, &["update-ref", "-d", &format!("{CHECKPOINT_NS}{ref_name}")]).map(drop)
}

// ---------------------------------------------------------------------------
// GitHub CLI
// ---------------------------------------------------------------------------

/// "2.103.3", or None if gh is not installed.
pub fn gh_version() -> Option<String> {
    let out = run(gh_cmd(None, ["--version"]), None).ok()?;
    lossy(&out).split_whitespace().nth(2).map(str::to_string)
}

/// Read-only (`gh auth status`, never `gh auth login`). Prefers the active account on
/// github.com. Falls back to parsing the text output on gh versions without `--json`.
pub fn gh_auth_status() -> GhAuth {
    const JQ: &str = r#".hosts | to_entries[] | .value[] | [.host, (.login // ""), .state, (.active | tostring), (.error // "")] | @tsv"#;
    let json = gh_cmd(None, ["auth", "status", "--no-refresh", "--json", "hosts", "--jq", JQ]).output();
    let out = match json {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return GhAuth::NotInstalled,
        Err(e) => return GhAuth::SignedOut(e.to_string()),
        Ok(out) => out,
    };
    if out.status.success() {
        return parse_gh_auth_tsv(&lossy(&out.stdout));
    }
    match gh_cmd(None, ["auth", "status"]).output() {
        Ok(out) => parse_gh_auth_text(&format!("{}{}", lossy(&out.stdout), lossy(&out.stderr))),
        Err(e) => GhAuth::SignedOut(e.to_string()),
    }
}

/// Lines of `host \t login \t state \t active \t error`.
fn parse_gh_auth_tsv(text: &str) -> GhAuth {
    let rows: Vec<Vec<&str>> = text.lines().map(|l| l.split('\t').collect()).collect();
    let ok = |r: &&Vec<&str>| r.len() >= 4 && r[2] == "success" && !r[1].is_empty();
    let best = rows.iter().filter(ok).min_by_key(|r| (r[3] != "true", r[0] != "github.com"));
    if let Some(r) = best {
        return GhAuth::SignedIn { user: r[1].to_string(), host: r[0].to_string() };
    }
    match rows.iter().find(|r| r.len() >= 3) {
        Some(r) => {
            let detail = r.get(4).filter(|e| !e.is_empty()).unwrap_or(&r[2]);
            GhAuth::SignedOut(format!("{}: {detail}", r[0]))
        }
        None => GhAuth::SignedOut("not logged in to any GitHub host".to_string()),
    }
}

/// "✓ Logged in to github.com account octocat (keyring)" (gh >= 2.40) or
/// "✓ Logged in to github.com as octocat (oauth_token)" (older). Active account is listed first.
fn parse_gh_auth_text(text: &str) -> GhAuth {
    for line in text.lines() {
        if let Some((_, rest)) = line.split_once("Logged in to ") {
            let mut words = rest.split_whitespace();
            if let (Some(host), Some(_), Some(user)) = (words.next(), words.next(), words.next()) {
                return GhAuth::SignedIn { user: user.to_string(), host: host.to_string() };
            }
        }
    }
    GhAuth::SignedOut(text.trim().to_string())
}

/// `gh pr create` with the body on stdin; returns the PR URL.
pub fn gh_pr_create(cwd: &Path, title: &str, body: &str, base: &str, head: &str, draft: bool) -> Result<String> {
    // `--flag=value` so values starting with '-' are never parsed as flags.
    let mut args = vec![
        "pr".to_string(),
        "create".to_string(),
        format!("--title={title}"),
        "--body-file=-".to_string(),
        format!("--base={base}"),
        format!("--head={head}"),
    ];
    if draft {
        args.push("--draft".to_string());
    }
    let out = lossy(&run(gh_cmd(Some(cwd), &args), Some(body.as_bytes()))?);
    Ok(out.lines().rev().map(str::trim).find(|l| l.starts_with("https://") || l.starts_with("http://")).unwrap_or(out.trim()).to_string())
}

pub fn gh_repo_default_branch(cwd: &Path) -> Option<String> {
    let out = run(gh_cmd(Some(cwd), ["repo", "view", "--json", "defaultBranchRef", "--jq", ".defaultBranchRef.name"]), None).ok()?;
    let name = lossy(&out).trim().to_string();
    (!name.is_empty()).then_some(name)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(dir: &Path, args: &[&str]) -> String {
        lossy(&git(dir, args).unwrap_or_else(|e| panic!("git {args:?}: {e}")))
    }

    /// Fresh repo on `main` with deterministic local config.
    fn init() -> (tempfile::TempDir, PathBuf) {
        let td = tempfile::tempdir().unwrap();
        let root = td.path().join("repo");
        fs::create_dir(&root).unwrap();
        sh(&root, &["init", "-q", "-b", "main"]);
        for (k, v) in
            [("user.name", "Test User"), ("user.email", "test@example.com"), ("commit.gpgsign", "false"), ("core.autocrlf", "false")]
        {
            sh(&root, &["config", k, v]);
        }
        (td, root)
    }

    fn write(root: &Path, rel: &str, content: impl AsRef<[u8]>) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, content).unwrap();
    }

    fn read(root: &Path, rel: &str) -> String {
        fs::read_to_string(root.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
    }

    fn commit(root: &Path, msg: &str) {
        sh(root, &["add", "-A"]);
        sh(root, &["commit", "-q", "-m", msg]);
    }

    fn numbered(range: std::ops::RangeInclusive<u32>) -> String {
        range.map(|i| format!("line {i}\n")).collect()
    }

    fn find<'a>(changes: &'a [FileChange], path: &str) -> &'a FileChange {
        changes.iter().find(|c| c.path == path).unwrap_or_else(|| panic!("{path} missing from {changes:#?}"))
    }

    #[test]
    fn basics_branches_and_push() {
        let (td, root) = init();
        assert!(git_version().unwrap().starts_with(char::is_numeric));
        assert_eq!(head_sha(&root).unwrap(), None);
        assert_eq!(current_branch(&root).unwrap().as_deref(), Some("main"));
        assert!(recent_log(&root, 5).unwrap().is_empty());
        assert!(repo_root(td.path()).is_err());

        write(&root, "sub/a.txt", "a\n");
        commit(&root, "init");
        let head = head_sha(&root).unwrap().unwrap();
        assert_eq!(rev_parse(&root, "HEAD").unwrap(), head);
        assert!(rev_parse(&root, "--nope").is_err());
        assert_eq!(fs::canonicalize(repo_root(&root.join("sub/a.txt")).unwrap()).unwrap(), fs::canonicalize(&root).unwrap());
        assert_eq!(default_base_branch(&root), "main");

        // A local bare "origin": push, then ahead/behind tracking.
        let remote = td.path().join("remote.git");
        sh(td.path(), &["init", "-q", "--bare", "-b", "main", "remote.git"]);
        sh(&root, &["remote", "add", "origin", remote.to_str().unwrap()]);
        push(&root, "main").unwrap();
        write(&root, "sub/a.txt", "a\nb\n");
        commit(&root, "second");
        let list = branches(&root).unwrap();
        let main = list.iter().find(|b| b.name == "main").unwrap();
        assert!(main.is_current && !main.is_remote);
        assert_eq!(main.upstream.as_deref(), Some("origin/main"));
        assert_eq!((main.ahead, main.behind), (1, 0));
        assert!(list.iter().any(|b| b.name == "origin/main" && b.is_remote));
        sh(&root, &["remote", "set-head", "origin", "main"]);
        let list = branches(&root).unwrap();
        assert_eq!(list.len(), 2); // origin/HEAD (a symref) is skipped
        assert_eq!(default_base_branch(&root), "main");
        sh(&root, &["branch", "-m", "main", "trunk"]);
        assert_eq!(default_base_branch(&root), "main"); // still origin/HEAD's target

        sh(&root, &["checkout", "-q", "--detach"]);
        assert_eq!(current_branch(&root).unwrap(), None);
    }

    #[test]
    fn status_reports_all_kinds() {
        let (_td, root) = init();
        write(&root, "mod.txt", "1\n2\n3\n");
        write(&root, "del.txt", "gone\n");
        write(&root, "old name.txt", numbered(1..=20));
        write(&root, "bin.dat", b"\x00\x01\x02");
        write(&root, ".gitignore", "*.log\n");
        commit(&root, "init");

        write(&root, "mod.txt", "1\ntwo\n3\n4\n");
        fs::remove_file(root.join("del.txt")).unwrap();
        sh(&root, &["mv", "old name.txt", "neu ä.txt"]);
        write(&root, "staged.txt", "s\n");
        sh(&root, &["add", "staged.txt"]);
        write(&root, "new dir/ünï cödé.txt", "a\nb"); // no trailing newline
        write(&root, "img.bin", b"PNG\x00\x00data\n");
        write(&root, "bin.dat", b"\x00\x01\x03");
        write(&root, "debug.log", "ignored\n");

        let st = status(&root.join("new dir")).unwrap(); // from a subdirectory
        let m = find(&st, "mod.txt");
        assert_eq!((m.kind, m.additions, m.deletions), (ChangeKind::Modified, 2, 1));
        let d = find(&st, "del.txt");
        assert_eq!((d.kind, d.deletions), (ChangeKind::Deleted, 1));
        let r = find(&st, "neu ä.txt");
        assert_eq!(r.kind, ChangeKind::Renamed);
        assert_eq!(r.old_path.as_deref(), Some("old name.txt"));
        assert_eq!((r.additions, r.deletions), (0, 0));
        let a = find(&st, "staged.txt");
        assert_eq!((a.kind, a.additions), (ChangeKind::Added, 1));
        let u = find(&st, "new dir/ünï cödé.txt");
        assert_eq!((u.kind, u.additions, u.binary), (ChangeKind::Untracked, 2, false));
        let ub = find(&st, "img.bin");
        assert_eq!((ub.kind, ub.binary, ub.additions), (ChangeKind::Untracked, true, 0));
        let b = find(&st, "bin.dat");
        assert_eq!((b.kind, b.binary), (ChangeKind::Modified, true));
        assert!(st.iter().all(|c| c.path != "debug.log"));
        assert_eq!(st.len(), 7);

        // changes_since an older base equals status here (HEAD is the only commit).
        assert_eq!(changes_since(&root, "HEAD").unwrap().len(), 7);
    }

    #[test]
    fn status_on_unborn_head() {
        let (_td, root) = init();
        write(&root, "a.txt", "x\n");
        sh(&root, &["add", "a.txt"]);
        write(&root, "b.txt", "y\ny\n");
        let st = status(&root).unwrap();
        assert_eq!(find(&st, "a.txt").kind, ChangeKind::Added);
        let b = find(&st, "b.txt");
        assert_eq!((b.kind, b.additions), (ChangeKind::Untracked, 2));
        let diffs = diff_worktree(&root, "HEAD").unwrap();
        assert_eq!(diffs.len(), 2);
        assert!(diffs.iter().any(|d| d.path == "a.txt" && d.kind == ChangeKind::Added));
    }

    #[test]
    fn status_marks_merge_conflicts() {
        let (_td, root) = init();
        write(&root, "c.txt", "base\n");
        commit(&root, "base");
        sh(&root, &["checkout", "-q", "-b", "other"]);
        write(&root, "c.txt", "theirs\n");
        commit(&root, "theirs");
        sh(&root, &["checkout", "-q", "main"]);
        write(&root, "c.txt", "ours\n");
        commit(&root, "ours");
        assert!(git(&root, &["merge", "-q", "other"]).is_err());
        let st = status(&root).unwrap();
        assert_eq!(st.len(), 1, "{st:#?}");
        assert_eq!(st[0].kind, ChangeKind::Conflicted);
    }

    #[test]
    fn autocrlf_revert_and_restore() {
        // Git for Windows' default config; worktree files are CRLF, blobs are LF.
        let (_td, root) = init();
        sh(&root, &["config", "core.autocrlf", "true"]);
        let orig = numbered(1..=20).replace('\n', "\r\n");
        write(&root, "f.txt", &orig);
        commit(&root, "init");
        let changed = orig.replace("line 2\r\n", "line two\r\n").replace("line 19\r\n", "line nineteen\r\n");
        write(&root, "f.txt", &changed);
        let cp = checkpoint_create(&root, "crlf", "changed").unwrap();

        let diffs = diff_worktree(&root, "HEAD").unwrap();
        assert!(!diffs[0].hunks[0].raw.contains('\r'));
        revert_hunk(&root, &diffs[0], 0).unwrap();
        assert_eq!(read(&root, "f.txt"), orig.replace("line 19\r\n", "line nineteen\r\n"));

        checkpoint_restore(&root, &cp, "crlf-safety").unwrap();
        assert_eq!(read(&root, "f.txt"), changed);
    }

    const FIXTURE: &str = concat!(
        "diff --git a/src/lib.rs b/src/lib.rs\n",
        "index 1111111..2222222 100644\n",
        "--- a/src/lib.rs\n",
        "+++ b/src/lib.rs\n",
        "@@ -1,4 +1,4 @@ fn main() {\n",
        " one\n",
        "-two\n",
        "+TWO\n",
        " three\n",
        "--- looks like a header\n",
        "+++ looks like a header\n",
        "@@ -10 +10 @@\n",
        "-ten\n",
        "+TEN\n",
        "diff --git a/old name.txt b/new name.txt\n",
        "similarity index 90%\n",
        "rename from old name.txt\n",
        "rename to new name.txt\n",
        "index 3333333..4444444 100644\n",
        "--- a/old name.txt\t\n",
        "+++ b/new name.txt\t\n",
        "@@ -1,2 +1,2 @@\n",
        " keep\n",
        "-last\n",
        "\\ No newline at end of file\n",
        "+last!\n",
        "\\ No newline at end of file\n",
        "diff --git a/img.png b/img.png\n",
        "new file mode 100644\n",
        "index 0000000..5555555\n",
        "Binary files /dev/null and b/img.png differ\n",
        "diff --git a/crlf.txt b/crlf.txt\n",
        "index 6666666..7777777 100644\n",
        "--- a/crlf.txt\n",
        "+++ b/crlf.txt\n",
        "@@ -1 +1,2 @@\n",
        " a\r\n",
        "+b\r\n",
        "diff --git a/gone.txt b/gone.txt\n",
        "deleted file mode 100644\n",
        "index 8888888..0000000\n",
        "--- a/gone.txt\n",
        "+++ /dev/null\n",
        "@@ -1 +0,0 @@\n",
        "-bye\n",
        "diff --git a/run me.sh b/run me.sh\n",
        "old mode 100644\n",
        "new mode 100755\n",
        "diff --git a/link b/link\n",
        "old mode 100644\n",
        "new mode 120000\n",
        "diff --git \"a/tab\\there \\\"q\\\".txt\" \"b/tab\\there \\\"q\\\".txt\"\n",
        "new file mode 100644\n",
        "index 0000000..9999999\n",
        "--- /dev/null\n",
        "+++ \"b/tab\\there \\\"q\\\".txt\"\n",
        "@@ -0,0 +1 @@\n",
        "+x\n",
        "diff --git \"a/caf\\303\\251.txt\" \"b/caf\\303\\251.txt\"\n",
        "index aaaaaaa..bbbbbbb 100644\n",
        "--- \"a/caf\\303\\251.txt\"\n",
        "+++ \"b/caf\\303\\251.txt\"\n",
        "@@ -1 +1 @@\n",
        "-x\n",
        "+y\n",
    );

    #[test]
    fn parse_fixture() {
        let files = parse_unified_diff(FIXTURE);
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            paths,
            ["src/lib.rs", "new name.txt", "img.png", "crlf.txt", "gone.txt", "run me.sh", "link", "tab\there \"q\".txt", "café.txt"]
        );

        let lib = &files[0];
        assert_eq!((lib.kind, lib.additions, lib.deletions), (ChangeKind::Modified, 3, 3));
        assert_eq!(lib.hunks.len(), 2);
        let h = &lib.hunks[0];
        assert_eq!(h.header, "@@ -1,4 +1,4 @@ fn main() {");
        assert_eq!((h.old_start, h.old_len, h.new_start, h.new_len), (1, 4, 1, 4));
        let kinds: Vec<LineKind> = h.lines.iter().map(|l| l.kind).collect();
        use LineKind::*;
        assert_eq!(kinds, [Context, Del, Add, Context, Del, Add]);
        assert_eq!(h.lines[4].text, "-- looks like a header");
        assert_eq!((h.lines[4].old_no, h.lines[4].new_no), (Some(4), None));
        assert_eq!((h.lines[5].old_no, h.lines[5].new_no), (None, Some(4)));
        assert_eq!((h.lines[3].old_no, h.lines[3].new_no), (Some(3), Some(3)));
        assert_eq!(lib.hunks[1].old_len, 1);

        let ren = &files[1];
        assert_eq!(ren.kind, ChangeKind::Renamed);
        assert_eq!(ren.old_path.as_deref(), Some("old name.txt"));
        assert_eq!((ren.additions, ren.deletions), (1, 1));
        assert_eq!(ren.hunks[0].lines.len(), 3);
        let patch = file_patch(ren, 0);
        assert!(patch.starts_with("diff --git a/new name.txt b/new name.txt\n"));
        assert!(patch.contains("-last\n\\ No newline at end of file\n+last!\n\\ No newline"));

        assert!(files[2].binary && files[2].kind == ChangeKind::Added && files[2].hunks.is_empty());

        let crlf = &files[3];
        assert_eq!(crlf.hunks[0].lines[1].text, "b");
        assert!(file_patch(crlf, 0).ends_with(" a\r\n+b\r\n"));

        assert_eq!((files[4].kind, files[4].deletions), (ChangeKind::Deleted, 1));
        assert_eq!(files[5].kind, ChangeKind::Modified);
        assert!(files[5].hunks.is_empty());
        assert_eq!(files[6].kind, ChangeKind::TypeChanged);
        assert_eq!(files[7].kind, ChangeKind::Added);
        // git appends a tab after names containing a space, quoted or not.
        assert!(file_patch(&files[7], 0).contains("+++ \"b/tab\\there \\\"q\\\".txt\"\t\n"));
        assert_eq!(files[8].additions, 1);
        assert!(file_patch(&files[8], 5).is_empty());
    }

    #[test]
    fn diff_worktree_includes_untracked_without_touching_index() {
        let (_td, root) = init();
        write(&root, "a.txt", "1\n2\n");
        commit(&root, "init");
        write(&root, "a.txt", "1\n2\n3\n");
        write(&root, "new file ü.txt", "x\ny\n");
        write(&root, "[ab].txt", "literal\n");
        let index_path = root.join(".git").join("index");
        let index_before = fs::read(&index_path).unwrap();

        let diffs = diff_worktree(&root, "HEAD").unwrap();
        let a = diffs.iter().find(|d| d.path == "a.txt").unwrap();
        assert_eq!((a.kind, a.additions), (ChangeKind::Modified, 1));
        let n = diffs.iter().find(|d| d.path == "new file ü.txt").unwrap();
        assert_eq!((n.kind, n.additions, n.hunks.len()), (ChangeKind::Untracked, 2, 1));
        assert!(diffs.iter().any(|d| d.path == "[ab].txt"));

        assert_eq!(fs::read(&index_path).unwrap(), index_before);
        assert_eq!(sh(&root, &["diff", "--cached", "--name-only"]), "");
        assert!(sh(&root, &["ls-files", "-o"]).contains("new file"));

        let revs = diff_revs(&root, "HEAD", "HEAD").unwrap();
        assert!(revs.is_empty());
    }

    #[test]
    fn slugify_cases() {
        let t = "Fix BPF capture filter on Windows!";
        assert_eq!(slugify(t, 60), "fix-bpf-capture-filter-on-windows");
        assert_eq!(slugify(t, 20), "fix-bpf-capture");
        assert_eq!(slugify(t, 15), "fix-bpf-capture");
        assert_eq!(slugify("Supercalifragilistic", 5), "super");
        assert_eq!(slugify("!!! ???", 30), "task");
        assert_eq!(slugify("", 30), "task");
        assert_eq!(slugify("Don't  stop__me--now ", 30), "dont-stop-me-now");
        assert_eq!(slugify("Größe ÄNDERN v2.0", 30), "grosse-andern-v2-0");
        assert_eq!(slugify("It’s “quoted” — ok", 30), "its-quoted-ok");
        assert_eq!(slugify("日本語", 30), "task");
    }

    #[test]
    fn unique_branch_names() {
        let (td, root) = init();
        write(&root, "a", "a\n");
        commit(&root, "init");
        assert_eq!(unique_branch_name(&root, "kiln", "foo"), "kiln/foo");
        sh(&root, &["branch", "kiln/foo"]);
        assert!(branch_exists(&root, "kiln/foo"));
        assert!(!branch_exists(&root, "kiln/fo"));
        assert_eq!(unique_branch_name(&root, "kiln/", "foo"), "kiln/foo-2");
        sh(&root, &["branch", "kiln/foo-2"]);
        assert_eq!(unique_branch_name(&root, "kiln", "foo"), "kiln/foo-3");
        assert_eq!(unique_branch_name(&root, "", "foo"), "foo", "empty prefix means no prefix");
        assert_eq!(unique_branch_name(&root, "agents", "foo"), "agents/foo");

        // An existing worktree directory named like the slug also counts as taken.
        worktree_add(&root, &td.path().join("wt").join("bar"), "other", "HEAD").unwrap();
        assert_eq!(unique_branch_name(&root, "kiln", "bar"), "kiln/bar-2");
    }

    #[test]
    fn worktree_lifecycle() {
        let (td, root) = init();
        write(&root, "a.txt", "a\n");
        commit(&root, "init");
        let head = head_sha(&root).unwrap();

        let wt = td.path().join("worktrees").join("feature ö one");
        worktree_add(&root, &wt, "kiln/feature-one", "HEAD").unwrap();
        let list = worktree_list(&root).unwrap();
        assert_eq!(list.len(), 2);
        assert!(list[0].is_main && !list[1].is_main);
        assert_eq!(list[0].branch.as_deref(), Some("main"));
        assert_eq!(list[1].branch.as_deref(), Some("kiln/feature-one"));
        assert_eq!(list[1].head, head);
        assert_eq!(fs::canonicalize(&list[1].path).unwrap(), fs::canonicalize(&wt).unwrap());
        assert_eq!(current_branch(&wt).unwrap().as_deref(), Some("kiln/feature-one"));
        // No upstream: an agent's bare `git push` must not target main.
        assert!(branches(&root).unwrap().iter().all(|b| b.upstream.is_none()));

        // Existing non-empty path is refused, and nothing is created.
        let busy = td.path().join("busy");
        write(&busy, "keep.txt", "mine");
        assert!(worktree_add(&root, &busy, "kiln/busy", "HEAD").is_err());
        assert!(!branch_exists(&root, "kiln/busy"));
        assert_eq!(read(&busy, "keep.txt"), "mine");

        // Dirty worktree (untracked file) is refused.
        write(&wt, "notes.txt", "wip");
        assert!(worktree_remove(&root, &wt).is_err());
        assert!(wt.join("notes.txt").exists());
        // Modified tracked file is refused too.
        fs::remove_file(wt.join("notes.txt")).unwrap();
        write(&wt, "a.txt", "changed\n");
        assert!(worktree_remove(&root, &wt).is_err());
        write(&wt, "a.txt", "a\n");

        worktree_remove(&root, &wt).unwrap();
        assert!(!wt.exists());
        assert_eq!(worktree_list(&root).unwrap().len(), 1);
        assert!(branch_exists(&root, "kiln/feature-one"));
    }

    #[test]
    fn commit_all_and_log() {
        let (_td, root) = init();
        write(&root, "a.txt", "1\n");
        let first = commit_all(&root, "first").unwrap();
        assert_eq!(head_sha(&root).unwrap().as_deref(), Some(first.as_str()));
        assert!(commit_all(&root, "nothing").is_err());

        write(&root, "a.txt", "1\n2\n");
        write(&root, "b c.txt", "b\n");
        let msg = "second: ünïcode\n\n# not a comment\nbody";
        let second = commit_all(&root.join("."), msg).unwrap();
        assert!(!is_dirty(&root).unwrap());
        assert_eq!(sh(&root, &["log", "-1", "--format=%B"]).trim(), msg);

        let log = recent_log(&root, 10).unwrap();
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].sha, second);
        assert_eq!(log[0].subject, "second: ünïcode");
        assert_eq!(log[0].author, "Test User");
        assert!(second.starts_with(&log[0].short));
        assert!(log[0].time_unix > 1_600_000_000);
        assert_eq!(log[1].sha, first);
        assert_eq!(recent_log(&root, 1).unwrap().len(), 1);

        let d = diff_revs(&root, &first, &second).unwrap();
        assert_eq!(d.len(), 2);
        assert_eq!(d.iter().find(|f| f.path == "b c.txt").unwrap().kind, ChangeKind::Added);
    }

    #[test]
    fn checkpoint_round_trip() {
        let (_td, root) = init();
        write(&root, "tracked.txt", "v1\n");
        write(&root, "remove me.txt", "bye\n");
        write(&root, "keep.txt", "keep\n");
        write(&root, ".gitignore", "*.log\n");
        commit(&root, "init");
        let head = head_sha(&root).unwrap().unwrap();
        let staged_before = sh(&root, &["ls-files", "--stage"]);

        // State A: modified, untracked, deleted, plus an ignored file.
        write(&root, "tracked.txt", "v2\n");
        write(&root, "untracked ä.txt", "ua\n");
        fs::remove_file(root.join("remove me.txt")).unwrap();
        write(&root, "debug.log", "log A\n");
        let cp = checkpoint_create(&root, "test/a", "state A").unwrap();
        assert_eq!(rev_parse(&root, "refs/kiln/checkpoints/test/a").unwrap(), cp);
        assert_eq!(rev_parse(&root, &format!("{cp}^")).unwrap(), head);
        let tree = sh(&root, &["ls-tree", "-r", "--name-only", &cp]);
        assert!(tree.contains("untracked ä.txt") && !tree.contains("debug.log"));
        assert!(!tree.contains("remove me.txt"));

        // State B: more changes after the checkpoint.
        write(&root, "tracked.txt", "v3\n");
        write(&root, "later.txt", "later\n");
        write(&root, "new dir/deep.txt", "deep\n");
        fs::remove_file(root.join("keep.txt")).unwrap();
        write(&root, "debug.log", "log B\n");

        let preview = checkpoint_changes(&root, &cp).unwrap();
        assert_eq!(find(&preview, "tracked.txt").kind, ChangeKind::Modified);
        assert_eq!(find(&preview, "later.txt").kind, ChangeKind::Deleted);
        assert_eq!(find(&preview, "new dir/deep.txt").kind, ChangeKind::Deleted);
        assert_eq!(find(&preview, "keep.txt").kind, ChangeKind::Added);
        assert_eq!(preview.len(), 4);

        let safety = checkpoint_restore(&root, &cp, "test/safety").unwrap();
        assert_eq!(read(&root, "tracked.txt"), "v2\n");
        assert_eq!(read(&root, "untracked ä.txt"), "ua\n");
        assert_eq!(read(&root, "keep.txt"), "keep\n");
        assert!(!root.join("remove me.txt").exists());
        assert!(!root.join("later.txt").exists());
        assert!(!root.join("new dir").exists());
        assert_eq!(read(&root, "debug.log"), "log B\n"); // ignored: untouched
        assert!(checkpoint_changes(&root, &cp).unwrap().is_empty());
        // HEAD and the real index are unchanged.
        assert_eq!(head_sha(&root).unwrap().unwrap(), head);
        assert_eq!(sh(&root, &["ls-files", "--stage"]), staged_before);
        assert_eq!(sh(&root, &["diff", "--cached", "--name-only"]), "");

        // The safety checkpoint brings state B back.
        checkpoint_restore(&root, &safety, "test/safety-2").unwrap();
        assert_eq!(read(&root, "tracked.txt"), "v3\n");
        assert_eq!(read(&root, "later.txt"), "later\n");
        assert_eq!(read(&root, "new dir/deep.txt"), "deep\n");
        assert!(!root.join("keep.txt").exists());
        assert_eq!(read(&root, "untracked ä.txt"), "ua\n");
        assert_eq!(read(&root, "debug.log"), "log B\n");
        assert_eq!(sh(&root, &["ls-files", "--stage"]), staged_before);

        checkpoint_delete(&root, "test/a").unwrap();
        assert!(rev_parse(&root, "refs/kiln/checkpoints/test/a").is_err());
    }

    #[test]
    fn checkpoint_on_unborn_repo() {
        let (_td, root) = init();
        write(&root, "a.txt", "a\n");
        let cp = checkpoint_create(&root, "first", "unborn").unwrap();
        assert!(rev_parse(&root, &format!("{cp}^")).is_err()); // no parent
        write(&root, "a.txt", "changed\n");
        write(&root, "b.txt", "b\n");
        checkpoint_restore(&root, &cp, "first-safety").unwrap();
        assert_eq!(read(&root, "a.txt"), "a\n");
        assert!(!root.join("b.txt").exists());
        assert_eq!(head_sha(&root).unwrap(), None);
    }

    #[test]
    fn revert_one_of_two_hunks() {
        let (_td, root) = init();
        let orig = numbered(1..=20);
        write(&root, "f.txt", &orig);
        commit(&root, "init");
        let changed = orig.replace("line 2\n", "line two\n").replace("line 19\n", "line nineteen\n");
        write(&root, "f.txt", &changed);

        let diffs = diff_worktree(&root, "HEAD").unwrap();
        let d = &diffs[0];
        assert_eq!(d.hunks.len(), 2);
        revert_hunk(&root, d, 1).unwrap();
        let expected = orig.replace("line 2\n", "line two\n");
        assert_eq!(read(&root, "f.txt"), expected);
        // Reverting the same hunk again no longer applies: error, file untouched.
        assert!(revert_hunk(&root, d, 1).is_err());
        assert_eq!(read(&root, "f.txt"), expected);
        assert_eq!(sh(&root, &["diff", "--cached", "--name-only"]), "");

        // Untracked files are refused.
        write(&root, "new.txt", "n\n");
        let diffs = diff_worktree(&root, "HEAD").unwrap();
        let n = diffs.iter().find(|d| d.path == "new.txt").unwrap();
        assert!(revert_hunk(&root, n, 0).is_err());
        assert!(root.join("new.txt").exists());
    }

    #[test]
    fn dirty_detection() {
        let (_td, root) = init();
        write(&root, "a.txt", "a\n");
        write(&root, ".gitignore", "*.log\n");
        commit(&root, "init");
        assert!(!is_dirty(&root).unwrap());
        write(&root, "x.log", "ignored");
        assert!(!is_dirty(&root).unwrap());
        write(&root, "new.txt", "n");
        assert!(is_dirty(&root).unwrap());
        fs::remove_file(root.join("new.txt")).unwrap();
        write(&root, "a.txt", "b\n");
        assert!(is_dirty(&root).unwrap());
    }

    #[test]
    fn gh_output_parsing() {
        if let Some(v) = gh_version() {
            assert!(v.starts_with(char::is_numeric), "{v}");
        }
        let tsv = "enterprise.example\tbob\tsuccess\ttrue\t\n\
                   github.com\talice-old\tsuccess\tfalse\t\n\
                   github.com\talice\tsuccess\ttrue\t\n";
        assert_eq!(parse_gh_auth_tsv(tsv), GhAuth::SignedIn { user: "alice".into(), host: "github.com".into() });
        assert_eq!(
            parse_gh_auth_tsv("github.com\talice\terror\ttrue\ttoken invalid\n"),
            GhAuth::SignedOut("github.com: token invalid".into())
        );
        assert!(matches!(parse_gh_auth_tsv(""), GhAuth::SignedOut(_)));
        let new = "github.com\n  ✓ Logged in to github.com account octo (keyring)\n  - Active account: true\n";
        let old = "github.com\n  ✓ Logged in to github.com as octo (oauth_token)\n";
        for text in [new, old] {
            assert_eq!(parse_gh_auth_text(text), GhAuth::SignedIn { user: "octo".into(), host: "github.com".into() });
        }
        assert!(matches!(
            parse_gh_auth_text("You are not logged into any GitHub hosts. To log in, run: gh auth login"),
            GhAuth::SignedOut(_)
        ));
    }
}
