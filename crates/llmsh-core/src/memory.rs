use anyhow::Context as _;
use rusqlite::{params, Connection};
use std::path::Path;
use std::sync::Mutex;

pub struct Memory {
    conn: Mutex<Connection>,
    in_memory: bool,
}

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

// ---------------------------------------------------------------------------
// v0.2.6 types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct ConversationMessage {
    pub id: i64, // 0 if not yet persisted
    pub ts: String,
    pub role: String, // 'user'|'assistant'|'tool'|'system'
    pub content: String,
    pub tool_call_id: Option<String>,
    pub name: Option<String>,
    pub tool_calls_json: Option<String>,
    pub insert_source: String, // 'turn' | 'compact' | 'compact_tail'
}

#[derive(Debug, Clone)]
pub struct Fact {
    pub id: i64,
    pub generation: i64,
    pub ts: String,
    pub category: String,
    pub claim: String,
    pub insert_source: String, // 'compact'|'manual'|'init'
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClearSource {
    ClearContext,
    ClearMemory,
    ClearAll,
    Compact,
    MemoryForget,
}

impl ClearSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ClearContext => "clear_context",
            Self::ClearMemory => "clear_memory",
            Self::ClearAll => "clear_all",
            Self::Compact => "compact",
            Self::MemoryForget => "memory_forget",
        }
    }
}

// ---------------------------------------------------------------------------
// Deprecated legacy stubs — removed in task 7
// ---------------------------------------------------------------------------

#[allow(deprecated)]
#[deprecated(note = "removed in task 7; use ConversationMessage instead")]
pub struct RecentAction {
    pub ts: String,
    pub kind: ActionKind,
    pub summary: String,
    pub detail_json: Option<String>,
}

#[allow(deprecated)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[deprecated(note = "removed in task 7; use ClearSource / ConversationMessage instead")]
pub enum ActionKind {
    UserInput,
    Assistant,
    Tool,
}

#[allow(deprecated)]
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

// ---------------------------------------------------------------------------

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

        // v1 — original tables.
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_version (
                version INTEGER PRIMARY KEY
            );
            CREATE TABLE IF NOT EXISTS init_audit (
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
            INSERT OR IGNORE INTO schema_version (version) VALUES (1);",
        )
        .context("run schema v1 migrations")?;

        let version: i64 = conn
            .query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0))
            .context("read schema_version")?;

        if version < 2 {
            conn.execute_batch(
                "DROP TABLE IF EXISTS recent_actions;

                CREATE TABLE IF NOT EXISTS conversation_messages (
                    id              INTEGER PRIMARY KEY AUTOINCREMENT,
                    ts              TEXT NOT NULL,
                    role            TEXT NOT NULL,
                    content         TEXT NOT NULL,
                    tool_call_id    TEXT,
                    name            TEXT,
                    tool_calls_json TEXT,
                    insert_source   TEXT NOT NULL,
                    cleared_at      TEXT,
                    cleared_source  TEXT
                );
                CREATE INDEX IF NOT EXISTS idx_conv_active
                    ON conversation_messages(cleared_at);

                CREATE TABLE IF NOT EXISTS long_term_facts (
                    id              INTEGER PRIMARY KEY AUTOINCREMENT,
                    generation      INTEGER NOT NULL,
                    ts              TEXT NOT NULL,
                    category        TEXT NOT NULL,
                    claim           TEXT NOT NULL,
                    insert_source   TEXT NOT NULL,
                    cleared_at      TEXT,
                    cleared_source  TEXT
                );
                CREATE INDEX IF NOT EXISTS idx_facts_active
                    ON long_term_facts(generation, cleared_at);

                INSERT INTO schema_version (version) VALUES (2);",
            )
            .context("run schema v2 migrations")?;
        }

        if version > 2 {
            anyhow::bail!(
                "memory db schema version {} is newer than supported (2)",
                version
            );
        }

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

    // -----------------------------------------------------------------------
    // Deprecated legacy stubs — TODO task 7: remove these
    // -----------------------------------------------------------------------

    #[deprecated(note = "no-op stub; removed in task 7")]
    #[allow(deprecated)]
    pub fn append_action(&self, _action: &RecentAction) -> anyhow::Result<()> {
        Ok(())
    }

    #[deprecated(note = "no-op stub; removed in task 7")]
    #[allow(deprecated)]
    pub fn last_actions(&self, _n: usize) -> anyhow::Result<Vec<RecentAction>> {
        Ok(vec![])
    }

    // -----------------------------------------------------------------------
    // Conversation messages (v0.2.6)
    // -----------------------------------------------------------------------

    pub fn append_message(&self, m: &ConversationMessage) -> anyhow::Result<i64> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("memory mutex poisoned"))?;
        conn.execute(
            "INSERT INTO conversation_messages \
             (ts, role, content, tool_call_id, name, tool_calls_json, insert_source) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                m.ts,
                m.role,
                m.content,
                m.tool_call_id,
                m.name,
                m.tool_calls_json,
                m.insert_source,
            ],
        )
        .context("append_message")?;
        Ok(conn.last_insert_rowid())
    }

    pub fn load_active_conversation(&self) -> anyhow::Result<Vec<ConversationMessage>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("memory mutex poisoned"))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, ts, role, content, tool_call_id, name, tool_calls_json, insert_source \
                 FROM conversation_messages \
                 WHERE cleared_at IS NULL \
                 ORDER BY id ASC",
            )
            .context("prepare load_active_conversation")?;
        let rows: Vec<ConversationMessage> = stmt
            .query_map([], |row| {
                Ok(ConversationMessage {
                    id: row.get(0)?,
                    ts: row.get(1)?,
                    role: row.get(2)?,
                    content: row.get(3)?,
                    tool_call_id: row.get(4)?,
                    name: row.get(5)?,
                    tool_calls_json: row.get(6)?,
                    insert_source: row.get(7)?,
                })
            })
            .context("query load_active_conversation")?
            .collect::<Result<Vec<_>, _>>()
            .context("collect load_active_conversation")?;
        Ok(rows)
    }

    pub fn mark_conversation_cleared(
        &self,
        ts: &str,
        source: ClearSource,
    ) -> anyhow::Result<usize> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("memory mutex poisoned"))?;
        let n = conn
            .execute(
                "UPDATE conversation_messages \
                 SET cleared_at = ?1, cleared_source = ?2 \
                 WHERE cleared_at IS NULL",
                params![ts, source.as_str()],
            )
            .context("mark_conversation_cleared")?;
        Ok(n)
    }

    pub fn mark_messages_cleared_by_ids(
        &self,
        ids: &[i64],
        ts: &str,
        source: ClearSource,
    ) -> anyhow::Result<usize> {
        if ids.is_empty() {
            return Ok(0);
        }
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("memory mutex poisoned"))?;
        let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "UPDATE conversation_messages SET cleared_at = ?, cleared_source = ? \
             WHERE id IN ({}) AND cleared_at IS NULL",
            placeholders
        );
        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = Vec::with_capacity(ids.len() + 2);
        params_vec.push(Box::new(ts.to_string()));
        params_vec.push(Box::new(source.as_str().to_string()));
        for id in ids {
            params_vec.push(Box::new(*id));
        }
        let refs: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|b| b.as_ref()).collect();
        let n = conn
            .execute(&sql, refs.as_slice())
            .context("mark_messages_cleared_by_ids")?;
        Ok(n)
    }

    // -----------------------------------------------------------------------
    // Long-term facts (v0.2.6)
    // -----------------------------------------------------------------------

    pub fn current_fact_generation(&self) -> anyhow::Result<i64> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("memory mutex poisoned"))?;
        let gen: Option<i64> = conn
            .query_row("SELECT MAX(generation) FROM long_term_facts", [], |r| {
                r.get(0)
            })
            .ok();
        Ok(gen.unwrap_or(0))
    }

    pub fn load_active_facts(&self) -> anyhow::Result<Vec<Fact>> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("memory mutex poisoned"))?;
        let mut stmt = conn
            .prepare(
                "SELECT id, generation, ts, category, claim, insert_source \
                 FROM long_term_facts \
                 WHERE cleared_at IS NULL \
                   AND generation = (SELECT COALESCE(MAX(generation), 0) FROM long_term_facts) \
                 ORDER BY id ASC",
            )
            .context("prepare load_active_facts")?;
        let rows: Vec<Fact> = stmt
            .query_map([], |row| {
                Ok(Fact {
                    id: row.get(0)?,
                    generation: row.get(1)?,
                    ts: row.get(2)?,
                    category: row.get(3)?,
                    claim: row.get(4)?,
                    insert_source: row.get(5)?,
                })
            })
            .context("query load_active_facts")?
            .collect::<Result<Vec<_>, _>>()
            .context("collect load_active_facts")?;
        Ok(rows)
    }

    /// Atomically replace the active facts with a new generation. Returns the
    /// new generation number.
    pub fn replace_facts_generation(
        &self,
        ts: &str,
        new_facts: &[(String, String)], // (category, claim)
    ) -> anyhow::Result<i64> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("memory mutex poisoned"))?;
        let tx = conn
            .transaction()
            .context("begin replace_facts_generation")?;
        let new_gen: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(generation), 0) + 1 FROM long_term_facts",
                [],
                |r| r.get(0),
            )
            .context("compute new generation")?;
        for (category, claim) in new_facts {
            tx.execute(
                "INSERT INTO long_term_facts \
                 (generation, ts, category, claim, insert_source) \
                 VALUES (?1, ?2, ?3, ?4, 'compact')",
                params![new_gen, ts, category, claim],
            )
            .context("insert fact")?;
        }
        tx.commit().context("commit replace_facts_generation")?;
        Ok(new_gen)
    }

    pub fn add_manual_fact(&self, ts: &str, category: &str, claim: &str) -> anyhow::Result<i64> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("memory mutex poisoned"))?;
        let tx = conn.transaction().context("begin add_manual_fact")?;
        let gen: i64 = tx
            .query_row(
                "SELECT COALESCE(MAX(generation), 0) FROM long_term_facts",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let target_gen = if gen == 0 { 1 } else { gen };
        tx.execute(
            "INSERT INTO long_term_facts \
             (generation, ts, category, claim, insert_source) \
             VALUES (?1, ?2, ?3, ?4, 'manual')",
            params![target_gen, ts, category, claim],
        )
        .context("insert manual fact")?;
        let id = tx.last_insert_rowid();
        tx.commit().context("commit add_manual_fact")?;
        Ok(id)
    }

    pub fn mark_facts_cleared(&self, ts: &str, source: ClearSource) -> anyhow::Result<usize> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("memory mutex poisoned"))?;
        let n = conn
            .execute(
                "UPDATE long_term_facts SET cleared_at = ?1, cleared_source = ?2 \
                 WHERE cleared_at IS NULL",
                params![ts, source.as_str()],
            )
            .context("mark_facts_cleared")?;
        Ok(n)
    }

    pub fn mark_fact_cleared_by_id(
        &self,
        id: i64,
        ts: &str,
        source: ClearSource,
    ) -> anyhow::Result<bool> {
        let conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("memory mutex poisoned"))?;
        let n = conn
            .execute(
                "UPDATE long_term_facts SET cleared_at = ?1, cleared_source = ?2 \
                 WHERE id = ?3 AND cleared_at IS NULL",
                params![ts, source.as_str(), id],
            )
            .context("mark_fact_cleared_by_id")?;
        Ok(n > 0)
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

    fn make_msg(role: &str, content: &str) -> ConversationMessage {
        ConversationMessage {
            id: 0,
            ts: now(),
            role: role.into(),
            content: content.into(),
            tool_call_id: None,
            name: None,
            tool_calls_json: None,
            insert_source: "turn".into(),
        }
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

    // -----------------------------------------------------------------------
    // v0.2.6 new tests
    // -----------------------------------------------------------------------

    #[test]
    fn append_and_load_active_conversation_roundtrip() {
        let m = Memory::open_in_memory().unwrap();
        m.append_message(&make_msg("user", "hello")).unwrap();
        m.append_message(&make_msg("assistant", "hi")).unwrap();
        let msgs = m.load_active_conversation().unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        assert_eq!(msgs[0].content, "hello");
    }

    #[test]
    fn mark_conversation_cleared_filters_subsequent_loads() {
        let m = Memory::open_in_memory().unwrap();
        m.append_message(&make_msg("user", "x")).unwrap();
        let cleared = m
            .mark_conversation_cleared(&now(), ClearSource::ClearContext)
            .unwrap();
        assert_eq!(cleared, 1);
        assert!(m.load_active_conversation().unwrap().is_empty());
        m.append_message(&make_msg("user", "y")).unwrap();
        assert_eq!(m.load_active_conversation().unwrap().len(), 1);
    }

    #[test]
    fn replace_facts_generation_bumps_and_loads_only_latest() {
        let m = Memory::open_in_memory().unwrap();
        let g1 = m
            .replace_facts_generation(
                &now(),
                &[
                    ("identity".into(), "user is alice".into()),
                    ("preference".into(), "likes terse output".into()),
                ],
            )
            .unwrap();
        assert_eq!(g1, 1);
        let facts = m.load_active_facts().unwrap();
        assert_eq!(facts.len(), 2);

        let g2 = m
            .replace_facts_generation(
                &now(),
                &[("identity".into(), "user is alice (verified)".into())],
            )
            .unwrap();
        assert_eq!(g2, 2);
        let facts = m.load_active_facts().unwrap();
        assert_eq!(facts.len(), 1);
        assert!(facts[0].claim.contains("verified"));
    }

    #[test]
    fn mark_fact_cleared_by_id_targets_single_row() {
        let m = Memory::open_in_memory().unwrap();
        m.replace_facts_generation(
            &now(),
            &[
                ("identity".into(), "claim 1".into()),
                ("identity".into(), "claim 2".into()),
            ],
        )
        .unwrap();
        let facts = m.load_active_facts().unwrap();
        assert_eq!(facts.len(), 2);
        let target = facts[0].id;
        let ok = m
            .mark_fact_cleared_by_id(target, &now(), ClearSource::MemoryForget)
            .unwrap();
        assert!(ok);
        let after = m.load_active_facts().unwrap();
        assert_eq!(after.len(), 1);
        assert_ne!(after[0].id, target);
    }

    #[test]
    fn add_manual_fact_works_on_empty_db() {
        let m = Memory::open_in_memory().unwrap();
        let id = m
            .add_manual_fact(&now(), "preference", "manual claim")
            .unwrap();
        assert!(id > 0);
        let facts = m.load_active_facts().unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].insert_source, "manual");
    }

    #[test]
    fn schema_v1_to_v2_preserves_init_audit() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("memory.db");
        {
            let conn = Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE schema_version (version INTEGER PRIMARY KEY);
                 CREATE TABLE init_audit (
                    id INTEGER PRIMARY KEY CHECK (id=1),
                    written_at TEXT NOT NULL, host TEXT NOT NULL, os TEXT NOT NULL,
                    kernel TEXT NOT NULL, user TEXT NOT NULL, home TEXT NOT NULL,
                    shell TEXT, summary_md TEXT NOT NULL
                 );
                 CREATE TABLE recent_actions (id INTEGER PRIMARY KEY, ts TEXT, kind TEXT, summary TEXT, detail_json TEXT);
                 INSERT INTO schema_version (version) VALUES (1);
                 INSERT INTO init_audit VALUES (1, '2026-01-01T00:00:00.000Z', 'h', 'o', 'k', 'u', '/h', NULL, 'old');",
            )
            .unwrap();
        }
        let m = Memory::open(&path).unwrap();
        let audit = m.read_init_audit().unwrap().unwrap();
        assert_eq!(audit.summary_md, "old");
        let conn = Connection::open(&path).unwrap();
        let exists: Option<String> = conn
            .query_row(
                "SELECT name FROM sqlite_master WHERE type='table' AND name='recent_actions'",
                [],
                |r| r.get(0),
            )
            .ok();
        assert!(exists.is_none(), "recent_actions should be dropped");
    }
}
