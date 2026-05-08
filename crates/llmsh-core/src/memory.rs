use crate::text::truncate_to_byte_budget;
use anyhow::Context as _;
use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::Mutex;

const SUMMARY_MAX_BYTES: usize = 200;

pub struct Memory {
    conn: Mutex<Connection>,
    in_memory: bool,
}

// Safety: rusqlite::Connection is !Send by default, but we gate all access
// behind a Mutex, so the combined type is Send + Sync.
unsafe impl Send for Memory {}
unsafe impl Sync for Memory {}

pub struct InitAudit {
    pub written_at: String,
    pub host: String,
    pub os: String,
    pub kernel: String,
    pub user: String,
    pub home: String,
    pub shell: Option<String>,
    pub summary_md: String,
}

pub struct RecentAction {
    pub ts: String,
    pub kind: ActionKind,
    pub summary: String,
    pub detail_json: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    UserInput,
    Assistant,
    Tool,
}

impl ActionKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ActionKind::UserInput => "user_input",
            ActionKind::Assistant => "assistant",
            ActionKind::Tool => "tool",
        }
    }

    pub fn parse_kind(s: &str) -> Option<ActionKind> {
        match s {
            "user_input" => Some(ActionKind::UserInput),
            "assistant" => Some(ActionKind::Assistant),
            "tool" => Some(ActionKind::Tool),
            _ => None,
        }
    }
}

impl Memory {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let is_new = !path.exists();

        if let Some(parent) = path.parent() {
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                std::fs::DirBuilder::new()
                    .recursive(true)
                    .mode(0o700)
                    .create(parent)
                    .with_context(|| format!("create memory dir {}", parent.display()))?;
            }
            #[cfg(not(unix))]
            {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("create memory dir {}", parent.display()))?;
            }
        }

        let conn =
            Connection::open(path).with_context(|| format!("open memory db {}", path.display()))?;

        #[cfg(unix)]
        if is_new {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
                .with_context(|| format!("set perms on {}", path.display()))?;
        }
        let _ = is_new;

        let mem = Self {
            conn: Mutex::new(conn),
            in_memory: false,
        };
        mem.migrate()?;
        mem.prune()?;
        Ok(mem)
    }

    pub fn open_in_memory() -> anyhow::Result<Self> {
        let conn = Connection::open_in_memory().context("open in-memory db")?;
        let mem = Self {
            conn: Mutex::new(conn),
            in_memory: true,
        };
        mem.migrate()?;
        Ok(mem)
    }

    fn migrate(&self) -> anyhow::Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("memory mutex poisoned"))?;

        let version: Option<i64> = conn
            .query_row(
                "SELECT name FROM sqlite_master WHERE type='table' AND name='schema_version'",
                [],
                |row| row.get(0),
            )
            .ok()
            .and_then(|_name: String| {
                conn.query_row("SELECT version FROM schema_version", [], |r| r.get(0))
                    .ok()
            });

        match version {
            None => {
                conn.execute_batch(
                    "CREATE TABLE schema_version (
                        version INTEGER PRIMARY KEY
                    );
                    CREATE TABLE init_audit (
                        id           INTEGER PRIMARY KEY CHECK (id = 1),
                        written_at   TEXT NOT NULL,
                        host         TEXT NOT NULL,
                        os           TEXT NOT NULL,
                        kernel       TEXT NOT NULL,
                        user         TEXT NOT NULL,
                        home         TEXT NOT NULL,
                        shell        TEXT,
                        summary_md   TEXT NOT NULL
                    );
                    CREATE TABLE recent_actions (
                        id           INTEGER PRIMARY KEY AUTOINCREMENT,
                        ts           TEXT NOT NULL,
                        kind         TEXT NOT NULL,
                        summary      TEXT NOT NULL,
                        detail_json  TEXT
                    );
                    CREATE INDEX idx_recent_actions_ts ON recent_actions(ts DESC);
                    INSERT INTO schema_version (version) VALUES (1);",
                )
                .context("run schema v1 migrations")?;
            }
            Some(1) => {}
            Some(v) => {
                anyhow::bail!("memory db schema version {} is newer than supported (1)", v);
            }
        }

        // WAL mode is not supported for in-memory databases.
        if !self.in_memory {
            conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
                .context("enable WAL")?;
            let mode: String = conn
                .query_row("PRAGMA journal_mode", [], |r| r.get(0))
                .context("verify WAL mode")?;
            anyhow::ensure!(mode == "wal", "expected WAL journal mode, got {}", mode);
        }

        Ok(())
    }

    fn prune(&self) -> anyhow::Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("memory mutex poisoned"))?;
        conn.execute(
            "DELETE FROM recent_actions WHERE id NOT IN \
             (SELECT id FROM recent_actions ORDER BY id DESC LIMIT 1000)",
            [],
        )
        .context("prune recent_actions")?;
        Ok(())
    }

    pub fn write_init_audit(&self, audit: &InitAudit) -> anyhow::Result<()> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("memory mutex poisoned"))?;
        conn.execute(
            "INSERT OR REPLACE INTO init_audit \
             (id, written_at, host, os, kernel, user, home, shell, summary_md) \
             VALUES (1, ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                audit.written_at,
                audit.host,
                audit.os,
                audit.kernel,
                audit.user,
                audit.home,
                audit.shell,
                audit.summary_md,
            ],
        )
        .context("write_init_audit")?;
        Ok(())
    }

    pub fn read_init_audit(&self) -> anyhow::Result<Option<InitAudit>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("memory mutex poisoned"))?;
        let result = conn.query_row(
            "SELECT written_at, host, os, kernel, user, home, shell, summary_md \
             FROM init_audit WHERE id = 1",
            [],
            |row| {
                Ok(InitAudit {
                    written_at: row.get(0)?,
                    host: row.get(1)?,
                    os: row.get(2)?,
                    kernel: row.get(3)?,
                    user: row.get(4)?,
                    home: row.get(5)?,
                    shell: row.get(6)?,
                    summary_md: row.get(7)?,
                })
            },
        );
        match result {
            Ok(a) => Ok(Some(a)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e).context("read_init_audit"),
        }
    }

    pub fn append_action(&self, action: &RecentAction) -> anyhow::Result<()> {
        let summary = truncate_to_byte_budget(&action.summary, SUMMARY_MAX_BYTES);
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("memory mutex poisoned"))?;
        conn.execute(
            "INSERT INTO recent_actions (ts, kind, summary, detail_json) VALUES (?1, ?2, ?3, ?4)",
            params![action.ts, action.kind.as_str(), summary, action.detail_json],
        )
        .context("append_action")?;
        Ok(())
    }

    pub fn last_actions(&self, n: usize) -> anyhow::Result<Vec<RecentAction>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("memory mutex poisoned"))?;
        let mut stmt = conn
            .prepare(
                "SELECT ts, kind, summary, detail_json \
                 FROM recent_actions ORDER BY id DESC LIMIT ?1",
            )
            .context("prepare last_actions")?;
        let mut rows: Vec<RecentAction> = stmt
            .query_map(params![n as i64], |row| {
                let kind_str: String = row.get(1)?;
                let kind = ActionKind::parse_kind(&kind_str).unwrap_or(ActionKind::UserInput);
                Ok(RecentAction {
                    ts: row.get(0)?,
                    kind,
                    summary: row.get(2)?,
                    detail_json: row.get(3)?,
                })
            })
            .context("query last_actions")?
            .collect::<Result<Vec<_>, _>>()
            .context("collect last_actions")?;
        rows.reverse();
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> String {
        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
    }

    fn make_audit() -> InitAudit {
        InitAudit {
            written_at: now(),
            host: "testhost".into(),
            os: "linux".into(),
            kernel: "6.1.0".into(),
            user: "alice".into(),
            home: "/home/alice".into(),
            shell: None,
            summary_md: "Test machine".into(),
        }
    }

    #[test]
    fn open_and_reopen_preserves_data() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("memory.db");

        {
            let m = Memory::open(&path).unwrap();
            m.write_init_audit(&make_audit()).unwrap();
            m.append_action(&RecentAction {
                ts: now(),
                kind: ActionKind::UserInput,
                summary: "hello".into(),
                detail_json: None,
            })
            .unwrap();
        }

        let m2 = Memory::open(&path).unwrap();
        assert!(m2.read_init_audit().unwrap().is_some());
        let actions = m2.last_actions(10).unwrap();
        assert_eq!(actions.len(), 1);
        assert_eq!(actions[0].summary, "hello");
    }

    #[test]
    fn migration_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("memory.db");
        Memory::open(&path).unwrap();
        Memory::open(&path).unwrap();
    }

    #[test]
    fn init_audit_round_trip_with_shell_none() {
        let m = Memory::open_in_memory().unwrap();
        let audit = make_audit();
        m.write_init_audit(&audit).unwrap();
        let got = m.read_init_audit().unwrap().unwrap();
        assert_eq!(got.host, "testhost");
        assert_eq!(got.user, "alice");
        assert!(got.shell.is_none());
        assert_eq!(got.summary_md, "Test machine");
    }

    #[test]
    fn init_audit_round_trip_with_shell_some() {
        let m = Memory::open_in_memory().unwrap();
        let audit = InitAudit {
            shell: Some("/bin/zsh".into()),
            ..make_audit()
        };
        m.write_init_audit(&audit).unwrap();
        let got = m.read_init_audit().unwrap().unwrap();
        assert_eq!(got.shell, Some("/bin/zsh".into()));
    }

    #[test]
    fn insert_or_replace_second_write_wins() {
        let m = Memory::open_in_memory().unwrap();
        m.write_init_audit(&InitAudit {
            summary_md: "first".into(),
            ..make_audit()
        })
        .unwrap();
        m.write_init_audit(&InitAudit {
            summary_md: "second".into(),
            ..make_audit()
        })
        .unwrap();
        let got = m.read_init_audit().unwrap().unwrap();
        assert_eq!(got.summary_md, "second");
    }

    #[test]
    fn read_init_audit_none_when_empty() {
        let m = Memory::open_in_memory().unwrap();
        assert!(m.read_init_audit().unwrap().is_none());
    }

    #[test]
    fn append_and_last_actions_chronological() {
        let m = Memory::open_in_memory().unwrap();
        for i in 0..5u32 {
            m.append_action(&RecentAction {
                ts: now(),
                kind: ActionKind::UserInput,
                summary: format!("msg {}", i),
                detail_json: None,
            })
            .unwrap();
        }
        let actions = m.last_actions(3).unwrap();
        assert_eq!(actions.len(), 3);
        assert_eq!(actions[0].summary, "msg 2");
        assert_eq!(actions[1].summary, "msg 3");
        assert_eq!(actions[2].summary, "msg 4");
    }

    #[test]
    fn last_actions_n_caps_result() {
        let m = Memory::open_in_memory().unwrap();
        for _ in 0..10 {
            m.append_action(&RecentAction {
                ts: now(),
                kind: ActionKind::Tool,
                summary: "x".into(),
                detail_json: None,
            })
            .unwrap();
        }
        assert_eq!(m.last_actions(4).unwrap().len(), 4);
        assert_eq!(m.last_actions(0).unwrap().len(), 0);
    }

    #[test]
    fn pruning_caps_at_1000() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("memory.db");

        {
            let m = Memory::open(&path).unwrap();
            for _ in 0..1050u32 {
                m.append_action(&RecentAction {
                    ts: now(),
                    kind: ActionKind::UserInput,
                    summary: "x".into(),
                    detail_json: None,
                })
                .unwrap();
            }
        }

        let m2 = Memory::open(&path).unwrap();
        let count = m2.last_actions(2000).unwrap().len();
        assert_eq!(count, 1000);
    }

    #[test]
    fn action_kind_round_trip() {
        for k in [
            ActionKind::UserInput,
            ActionKind::Assistant,
            ActionKind::Tool,
        ] {
            let s = k.as_str();
            assert_eq!(ActionKind::parse_kind(s), Some(k));
        }
    }

    #[test]
    fn summary_truncation_multibyte() {
        let m = Memory::open_in_memory().unwrap();
        let long_summary = "€".repeat(100);
        assert!(long_summary.len() > 200);
        m.append_action(&RecentAction {
            ts: now(),
            kind: ActionKind::UserInput,
            summary: long_summary,
            detail_json: None,
        })
        .unwrap();
        let actions = m.last_actions(1).unwrap();
        assert!(actions[0].summary.len() <= 200);
        assert!(std::str::from_utf8(actions[0].summary.as_bytes()).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn file_permissions_0600_dir_0700() {
        use std::os::unix::fs::MetadataExt;
        let tmp = tempfile::tempdir().unwrap();
        let db_dir = tmp.path().join("subdir");
        let path = db_dir.join("memory.db");
        Memory::open(&path).unwrap();
        let dir_meta = std::fs::metadata(&db_dir).unwrap();
        let file_meta = std::fs::metadata(&path).unwrap();
        assert_eq!(dir_meta.mode() & 0o777, 0o700, "dir should be 0700");
        assert_eq!(file_meta.mode() & 0o777, 0o600, "file should be 0600");
    }
}
