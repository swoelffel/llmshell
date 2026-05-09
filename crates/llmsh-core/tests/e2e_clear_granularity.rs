//! End-to-end coverage for the soft-delete granularity matrix.
//!
//! Each `/clear-*` variant must clear exactly the right subset of data:
//!
//! | command          | conversation | facts |
//! |------------------|:------------:|:-----:|
//! | /clear-context   |     yes      |  no   |
//! | /clear-memory    |      no      |  yes  |
//! | /clear-all       |     yes      |  yes  |
//!
//! Deleted rows are never physically removed — they are soft-deleted
//! (cleared_at / cleared_source set) so the audit trail is preserved.

use llmsh_core::memory::{ClearSource, ConversationMessage, Memory};

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
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

/// Seed a fresh in-memory DB with 2 conversation messages and 2 facts.
/// Returns the opened `Memory`.
fn seeded_memory() -> Memory {
    let mem = Memory::open_in_memory().unwrap();
    mem.append_message(&make_msg("user", "x")).unwrap();
    mem.append_message(&make_msg("assistant", "y")).unwrap();
    mem.replace_facts_generation(
        &now(),
        &[
            ("identity".into(), "claim a".into()),
            ("preference".into(), "claim b".into()),
        ],
    )
    .unwrap();
    mem
}

/// `/clear-context`: conversation is cleared; facts survive.
#[test]
fn clear_context_removes_messages_keeps_facts() {
    let mem = seeded_memory();

    mem.mark_conversation_cleared(&now(), ClearSource::ClearContext)
        .unwrap();

    assert!(
        mem.load_active_conversation().unwrap().is_empty(),
        "/clear-context must wipe active conversation"
    );
    assert_eq!(
        mem.load_active_facts().unwrap().len(),
        2,
        "/clear-context must leave facts intact"
    );
}

/// `/clear-memory`: facts are cleared; conversation survives.
#[test]
fn clear_memory_removes_facts_keeps_messages() {
    let mem = seeded_memory();

    mem.mark_facts_cleared(&now(), ClearSource::ClearMemory)
        .unwrap();

    assert_eq!(
        mem.load_active_conversation().unwrap().len(),
        2,
        "/clear-memory must leave conversation intact"
    );
    assert!(
        mem.load_active_facts().unwrap().is_empty(),
        "/clear-memory must wipe active facts"
    );
}

/// `/clear-all`: both conversation and facts are cleared.
#[test]
fn clear_all_removes_both_messages_and_facts() {
    let mem = seeded_memory();

    mem.mark_conversation_cleared(&now(), ClearSource::ClearAll)
        .unwrap();
    mem.mark_facts_cleared(&now(), ClearSource::ClearAll)
        .unwrap();

    assert!(
        mem.load_active_conversation().unwrap().is_empty(),
        "/clear-all must wipe active conversation"
    );
    assert!(
        mem.load_active_facts().unwrap().is_empty(),
        "/clear-all must wipe active facts"
    );
}

/// Forensic preservation: after `/clear-context` the rows are NOT physically
/// deleted — they remain in the DB with `cleared_at` set. A subsequent open
/// of the same file-backed DB must still see 2 rows with `cleared_at IS NOT
/// NULL` when queried directly via a second `rusqlite::Connection`.
#[test]
fn clear_context_preserves_rows_in_db_forensically() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("memory.db");

    {
        let mem = Memory::open(&db_path).unwrap();
        mem.append_message(&make_msg("user", "first")).unwrap();
        mem.append_message(&make_msg("assistant", "second"))
            .unwrap();

        // Clear — rows must be soft-deleted, not dropped.
        mem.mark_conversation_cleared(&now(), ClearSource::ClearContext)
            .unwrap();

        // Active view is empty.
        assert!(
            mem.load_active_conversation().unwrap().is_empty(),
            "active conversation must be empty after clear"
        );
    }

    // Open the SQLite file with a raw connection to count soft-deleted rows.
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    let cleared_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM conversation_messages WHERE cleared_at IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap();

    assert_eq!(
        cleared_count, 2,
        "both rows must be preserved in the DB with cleared_at set (forensic audit trail)"
    );
}

/// Clearing facts then inserting new ones via `replace_facts_generation` must
/// produce fresh facts visible via `load_active_facts`, independent of the
/// cleared rows.
#[test]
fn clear_memory_then_add_facts_yields_fresh_generation() {
    let mem = seeded_memory();

    mem.mark_facts_cleared(&now(), ClearSource::ClearMemory)
        .unwrap();

    // Facts should be gone.
    assert!(mem.load_active_facts().unwrap().is_empty());

    // Add a new generation.
    let gen = mem
        .replace_facts_generation(&now(), &[("identity".into(), "new claim".into())])
        .unwrap();
    assert!(
        gen >= 2,
        "new generation must be higher than the cleared one"
    );

    let facts = mem.load_active_facts().unwrap();
    assert_eq!(facts.len(), 1);
    assert_eq!(facts[0].claim, "new claim");
}
