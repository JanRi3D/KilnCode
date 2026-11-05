//! Local data locations and a tiny redacting file logger.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

/// `%APPDATA%\Kiln\Kiln Code\data` on Windows, XDG/Library equivalents elsewhere.
/// `KILN_DATA_DIR` overrides it (used for demo mode and tests).
pub fn data_dir() -> PathBuf {
    if let Ok(d) = std::env::var("KILN_DATA_DIR") {
        return PathBuf::from(d);
    }
    directories::ProjectDirs::from("dev", "Kiln", "Kiln Code")
        .map(|d| d.data_dir().to_path_buf())
        .unwrap_or_else(|| std::env::temp_dir().join("kiln-code"))
}
pub fn logs_dir() -> PathBuf {
    data_dir().join("logs")
}
pub fn default_worktree_root() -> PathBuf {
    directories::BaseDirs::new().map(|b| b.home_dir().join(".kiln").join("worktrees")).unwrap_or_else(|| data_dir().join("worktrees"))
}
pub fn home_dir() -> PathBuf {
    directories::BaseDirs::new().map(|b| b.home_dir().to_path_buf()).unwrap_or_default()
}

static LOG: Mutex<Option<std::fs::File>> = Mutex::new(None);

pub fn init() {
    let dir = logs_dir();
    let _ = std::fs::create_dir_all(&dir);
    let day = crate::model::now() / 86_400;
    if let Ok(f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join(format!("kiln-{day}.log")))
        && let Ok(mut g) = LOG.lock()
    {
        *g = Some(f);
    }
}

/// Removes rotated log files older than `days`. Returns bytes freed.
pub fn prune(days: u32) -> u64 {
    let cutoff = std::time::SystemTime::now() - std::time::Duration::from_secs(days as u64 * 86_400);
    let today = format!("kiln-{}.log", crate::model::now() / 86_400);
    let mut freed = 0;
    if let Ok(rd) = std::fs::read_dir(logs_dir()) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            if name == today || !name.starts_with("kiln-") {
                continue;
            }
            if let Ok(md) = e.metadata()
                && md.modified().map(|m| m < cutoff).unwrap_or(false)
                && std::fs::remove_file(e.path()).is_ok()
            {
                freed += md.len();
            }
        }
    }
    freed
}

/// Masks things that look like credentials before they reach disk.
pub fn redact(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for word in s.split_inclusive(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == ',') {
        let core = word.trim_end_matches(|c: char| c.is_whitespace() || c == '"' || c == '\'' || c == ',');
        let tail = &word[core.len()..];
        let secret = ["sk-", "ghp_", "gho_", "ghs_", "github_pat_", "xox", "eyJ"].iter().any(|p| core.starts_with(p) && core.len() > 16);
        if secret {
            out.push_str("[redacted]");
            out.push_str(tail);
        } else {
            out.push_str(word);
        }
    }
    for key in ["access_token", "refresh_token", "api_key", "apiKey", "authorization", "Authorization", "id_token"] {
        let pat = format!("\"{key}\":\"");
        let mut from = 0;
        while let Some(i) = out[from..].find(&pat) {
            let start = from + i + pat.len();
            let end = out[start..].find('"').map(|e| start + e).unwrap_or(out.len());
            out.replace_range(start..end, "[redacted]");
            from = start + "[redacted]".len();
        }
    }
    out
}

fn write(level: &str, msg: &str) {
    let line = format!("{} {level} {}\n", crate::model::now(), redact(msg));
    if let Ok(mut g) = LOG.lock()
        && let Some(f) = g.as_mut()
    {
        let _ = f.write_all(line.as_bytes());
        return;
    }
    eprint!("{line}");
}
pub fn info(msg: &str) {
    write("INFO", msg);
}
pub fn error(msg: &str) {
    write("ERROR", msg);
}

#[cfg(test)]
mod tests {
    #[test]
    fn redacts_tokens() {
        let r = super::redact(r#"token ghp_abcdefghijklmnopqrstuvwxyz and {"access_token":"abc.def"} ok"#);
        assert!(!r.contains("ghp_abcdef"));
        assert!(!r.contains("abc.def"));
        assert!(r.contains("ok"));
        let two = super::redact(r#"{"access_token":"aaa111","x":1,"access_token":"bbb222"}"#);
        assert!(!two.contains("aaa111") && !two.contains("bbb222"), "{two}");
    }
}
