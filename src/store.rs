//! Durable local persistence (SQLite, WAL) with versioned migrations.
//! Writes go through a dedicated writer thread so the UI never blocks on disk;
//! reads happen at startup and when a thread's history is first opened.

use crate::model::*;
use crate::settings::Settings;
use rusqlite::{Connection, OptionalExtension, params};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

const MIGRATIONS: &[&str] = &[
    // v1
    "CREATE TABLE projects (id TEXT PRIMARY KEY, name TEXT NOT NULL, path TEXT NOT NULL UNIQUE, pinned INTEGER NOT NULL DEFAULT 0,
        archived INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL, last_used_at INTEGER NOT NULL, data TEXT NOT NULL DEFAULT '{}');
     CREATE TABLE threads (id TEXT PRIMARY KEY, project_id TEXT, state TEXT NOT NULL, archived INTEGER NOT NULL DEFAULT 0,
        updated_at INTEGER NOT NULL, data TEXT NOT NULL);
     CREATE INDEX threads_project ON threads(project_id);
     CREATE TABLE items (id TEXT PRIMARY KEY, thread_id TEXT NOT NULL, seq INTEGER NOT NULL, created_at INTEGER NOT NULL, kind TEXT NOT NULL);
     CREATE INDEX items_thread ON items(thread_id, seq);
     CREATE TABLE rules (id TEXT PRIMARY KEY, data TEXT NOT NULL);
     CREATE TABLE kv (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
    // v2: review state and checkpoints
    "CREATE TABLE comments (id TEXT PRIMARY KEY, thread_id TEXT NOT NULL, data TEXT NOT NULL);
     CREATE INDEX comments_thread ON comments(thread_id);
     CREATE TABLE viewed (thread_id TEXT NOT NULL, path TEXT NOT NULL, fingerprint TEXT NOT NULL, PRIMARY KEY(thread_id, path));
     CREATE TABLE checkpoints (id TEXT PRIMARY KEY, thread_id TEXT NOT NULL, created_at INTEGER NOT NULL, data TEXT NOT NULL);
     CREATE INDEX checkpoints_thread ON checkpoints(thread_id, created_at);",
];

pub fn migrate(conn: &Connection) -> rusqlite::Result<u32> {
    let current: u32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", (i + 1) as u32)?;
        tx.commit()?;
    }
    Ok(MIGRATIONS.len() as u32)
}

pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    migrate(&conn)?;
    Ok(conn)
}

type Job = Box<dyn FnOnce(&Connection) -> rusqlite::Result<()> + Send>;

/// Handle to the database: a background writer plus a read connection.
pub struct Store {
    tx: mpsc::Sender<Job>,
    read: Connection,
    pub path: PathBuf,
}

impl Store {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let read = open(path)?;
        let writer = open(path)?;
        let (tx, rx) = mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("kiln-db".into())
            .spawn(move || {
                for job in rx {
                    if let Err(e) = job(&writer) {
                        crate::log::error(&format!("db write failed: {e}"));
                    }
                }
            })
            .expect("spawn db thread");
        Ok(Self { tx, read, path: path.to_owned() })
    }

    fn write(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<()> + Send + 'static) {
        let _ = self.tx.send(Box::new(f));
    }

    /// Blocks until all queued writes are applied (used on shutdown and in tests).
    pub fn flush(&self) {
        let (done_tx, done_rx) = mpsc::channel();
        self.write(move |_| {
            let _ = done_tx.send(());
            Ok(())
        });
        let _ = done_rx.recv_timeout(std::time::Duration::from_secs(5));
    }

    // ---- kv / settings ----
    pub fn get_kv(&self, key: &str) -> Option<String> {
        self.read.query_row("SELECT value FROM kv WHERE key=?1", [key], |r| r.get(0)).optional().ok().flatten()
    }
    pub fn set_kv(&self, key: &str, value: String) {
        let key = key.to_owned();
        self.write(move |c| {
            c.execute("INSERT INTO kv(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![key, value])
                .map(|_| ())
        });
    }
    pub fn load_settings(&self) -> Settings {
        self.get_kv("settings").and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }
    pub fn save_settings(&self, s: &Settings) {
        if let Ok(json) = serde_json::to_string(s) {
            self.set_kv("settings", json);
        }
    }

    // ---- projects ----
    pub fn load_projects(&self) -> Vec<Project> {
        let mut st = match self.read.prepare("SELECT id,name,path,pinned,archived,created_at,last_used_at,data FROM projects") {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        st.query_map([], |r| {
            let data: String = r.get(7)?;
            let extra: serde_json::Value = serde_json::from_str(&data).unwrap_or_default();
            Ok(Project {
                id: r.get(0)?,
                name: r.get(1)?,
                path: PathBuf::from(r.get::<_, String>(2)?),
                pinned: r.get::<_, i64>(3)? != 0,
                archived: r.get::<_, i64>(4)? != 0,
                created_at: r.get(5)?,
                last_used_at: r.get(6)?,
                scripts: serde_json::from_value(extra.get("scripts").cloned().unwrap_or_default()).unwrap_or_default(),
            })
        })
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
    }
    pub fn save_project(&self, p: &Project) {
        let p = p.clone();
        self.write(move |c| {
            let data = serde_json::json!({ "scripts": p.scripts }).to_string();
            c.execute(
                "INSERT INTO projects(id,name,path,pinned,archived,created_at,last_used_at,data) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
                 ON CONFLICT(id) DO UPDATE SET name=excluded.name,path=excluded.path,pinned=excluded.pinned,archived=excluded.archived,
                 last_used_at=excluded.last_used_at,data=excluded.data",
                params![p.id, p.name, p.path.to_string_lossy(), p.pinned as i64, p.archived as i64, p.created_at, p.last_used_at, data],
            )
            .map(|_| ())
        });
    }
    pub fn delete_project(&self, id: &str) {
        let id = id.to_owned();
        self.write(move |c| {
            c.execute("DELETE FROM projects WHERE id=?1", [&id])?;
            c.execute("UPDATE threads SET project_id=NULL WHERE project_id=?1", [&id]).map(|_| ())
        });
    }

    // ---- threads ----
    pub fn load_threads(&self) -> Vec<Thread> {
        let mut st = match self.read.prepare("SELECT data FROM threads") {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        st.query_map([], |r| r.get::<_, String>(0))
            .map(|rows| rows.flatten().filter_map(|d| serde_json::from_str(&d).ok()).collect())
            .unwrap_or_default()
    }
    pub fn save_thread(&self, t: &Thread) {
        let Ok(data) = serde_json::to_string(t) else { return };
        let (id, project, state, archived, updated) = (t.id.clone(), t.project_id.clone(), t.state.key(), t.archived as i64, t.updated_at);
        self.write(move |c| {
            c.execute(
                "INSERT INTO threads(id,project_id,state,archived,updated_at,data) VALUES(?1,?2,?3,?4,?5,?6)
                 ON CONFLICT(id) DO UPDATE SET project_id=excluded.project_id,state=excluded.state,archived=excluded.archived,
                 updated_at=excluded.updated_at,data=excluded.data",
                params![id, project, state, archived, updated, data],
            )
            .map(|_| ())
        });
    }
    pub fn delete_thread(&self, id: &str) {
        let id = id.to_owned();
        self.write(move |c| {
            for table in ["items", "comments", "viewed", "checkpoints"] {
                c.execute(&format!("DELETE FROM {table} WHERE thread_id=?1"), [&id])?;
            }
            c.execute("DELETE FROM threads WHERE id=?1", [&id]).map(|_| ())
        });
    }

    // ---- items ----
    pub fn load_items(&self, thread_id: &str) -> Vec<Item> {
        let mut st = match self.read.prepare("SELECT id,seq,created_at,kind FROM items WHERE thread_id=?1 ORDER BY seq") {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        st.query_map([thread_id], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, i64>(2)?, r.get::<_, String>(3)?)))
            .map(|rows| {
                rows.flatten()
                    .filter_map(|(id, seq, created_at, kind)| {
                        serde_json::from_str(&kind).ok().map(|kind| Item { id, thread_id: thread_id.to_owned(), seq, created_at, kind })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
    pub fn save_item(&self, it: &Item) {
        let Ok(kind) = serde_json::to_string(&it.kind) else { return };
        let (id, tid, seq, at) = (it.id.clone(), it.thread_id.clone(), it.seq, it.created_at);
        self.write(move |c| {
            c.execute(
                "INSERT INTO items(id,thread_id,seq,created_at,kind) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(id) DO UPDATE SET kind=excluded.kind",
                params![id, tid, seq, at, kind],
            )
            .map(|_| ())
        });
    }

    // ---- rules ----
    pub fn load_rules(&self) -> Vec<ApprovalRule> {
        let mut st = match self.read.prepare("SELECT data FROM rules") {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        st.query_map([], |r| r.get::<_, String>(0))
            .map(|rows| rows.flatten().filter_map(|d| serde_json::from_str(&d).ok()).collect())
            .unwrap_or_default()
    }
    pub fn save_rule(&self, rule: &ApprovalRule) {
        let Ok(data) = serde_json::to_string(rule) else { return };
        let id = rule.id.clone();
        self.write(move |c| {
            c.execute("INSERT INTO rules(id,data) VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET data=excluded.data", params![id, data])
                .map(|_| ())
        });
    }
    pub fn delete_rule(&self, id: &str) {
        let id = id.to_owned();
        self.write(move |c| c.execute("DELETE FROM rules WHERE id=?1", [id]).map(|_| ()));
    }

    // ---- review ----
    pub fn load_comments(&self, thread_id: &str) -> Vec<ReviewComment> {
        let mut st = match self.read.prepare("SELECT data FROM comments WHERE thread_id=?1") {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        st.query_map([thread_id], |r| r.get::<_, String>(0))
            .map(|rows| rows.flatten().filter_map(|d| serde_json::from_str(&d).ok()).collect())
            .unwrap_or_default()
    }
    pub fn save_comment(&self, cm: &ReviewComment) {
        let Ok(data) = serde_json::to_string(cm) else { return };
        let (id, tid) = (cm.id.clone(), cm.thread_id.clone());
        self.write(move |c| {
            c.execute(
                "INSERT INTO comments(id,thread_id,data) VALUES(?1,?2,?3) ON CONFLICT(id) DO UPDATE SET data=excluded.data",
                params![id, tid, data],
            )
            .map(|_| ())
        });
    }
    pub fn load_viewed(&self, thread_id: &str) -> Vec<(String, String)> {
        let mut st = match self.read.prepare("SELECT path,fingerprint FROM viewed WHERE thread_id=?1") {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        st.query_map([thread_id], |r| Ok((r.get(0)?, r.get(1)?))).map(|rows| rows.flatten().collect()).unwrap_or_default()
    }
    pub fn set_viewed(&self, thread_id: &str, path: &str, fingerprint: Option<String>) {
        let (tid, path) = (thread_id.to_owned(), path.to_owned());
        self.write(move |c| match fingerprint {
            Some(fp) => c
                .execute("INSERT INTO viewed(thread_id,path,fingerprint) VALUES(?1,?2,?3) ON CONFLICT(thread_id,path) DO UPDATE SET fingerprint=excluded.fingerprint", params![tid, path, fp])
                .map(|_| ()),
            None => c.execute("DELETE FROM viewed WHERE thread_id=?1 AND path=?2", params![tid, path]).map(|_| ()),
        });
    }

    // ---- checkpoints ----
    pub fn load_checkpoints(&self, thread_id: &str) -> Vec<Checkpoint> {
        let mut st = match self.read.prepare("SELECT data FROM checkpoints WHERE thread_id=?1 ORDER BY created_at") {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        st.query_map([thread_id], |r| r.get::<_, String>(0))
            .map(|rows| rows.flatten().filter_map(|d| serde_json::from_str(&d).ok()).collect())
            .unwrap_or_default()
    }
    pub fn save_checkpoint(&self, cp: &Checkpoint) {
        let Ok(data) = serde_json::to_string(cp) else { return };
        let (id, tid, at) = (cp.id.clone(), cp.thread_id.clone(), cp.created_at);
        self.write(move |c| {
            c.execute("INSERT INTO checkpoints(id,thread_id,created_at,data) VALUES(?1,?2,?3,?4) ON CONFLICT(id) DO UPDATE SET data=excluded.data", params![id, tid, at, data]).map(|_| ())
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrations_apply_once_and_are_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kiln.db");
        let c = open(&path).unwrap();
        let v: u32 = c.pragma_query_value(None, "user_version", |r| r.get(0)).unwrap();
        assert_eq!(v as usize, MIGRATIONS.len());
        drop(c);
        // Re-opening must not re-run migrations (would fail on CREATE TABLE).
        let c = open(&path).unwrap();
        let n: i64 = c.query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name='checkpoints'", [], |r| r.get(0)).unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn upgrades_a_v1_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kiln.db");
        {
            let c = Connection::open(&path).unwrap();
            c.execute_batch(MIGRATIONS[0]).unwrap();
            c.pragma_update(None, "user_version", 1).unwrap();
            c.execute("INSERT INTO kv(key,value) VALUES('x','kept')", []).unwrap();
        }
        let c = open(&path).unwrap();
        let kept: String = c.query_row("SELECT value FROM kv WHERE key='x'", [], |r| r.get(0)).unwrap();
        assert_eq!(kept, "kept");
        c.execute("INSERT INTO viewed(thread_id,path,fingerprint) VALUES('t','p','f')", []).unwrap();
    }

    #[test]
    fn threads_items_settings_and_session_ids_persist() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("kiln.db");
        let mut t = Thread::new(None, "Fix it".into(), ProviderKind::Codex, "gpt".into());
        t.native_session_id = Some("019a-native".into());
        t.state = RunState::Working;
        {
            let s = Store::open(&path).unwrap();
            s.save_thread(&t);
            s.save_item(&Item {
                id: "i1".into(),
                thread_id: t.id.clone(),
                seq: 1,
                created_at: 1,
                kind: ItemKind::User { text: "hi".into(), attachments: vec![] },
            });
            let st = Settings { theme: "Light".into(), ..Default::default() };
            s.save_settings(&st);
            s.flush();
        }
        let s = Store::open(&path).unwrap();
        let loaded = s.load_threads();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].native_session_id.as_deref(), Some("019a-native"));
        assert_ne!(loaded[0].id, "019a-native", "Kiln id and provider id stay separate");
        assert_eq!(loaded[0].state.after_restart(), RunState::Cancelled);
        assert_eq!(s.load_items(&t.id).len(), 1);
        assert_eq!(s.load_settings().theme, "Light");
    }
}
