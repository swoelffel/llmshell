//! End-to-end coverage for conversation persistence across session boundaries.
//!
//! These tests exercise `Memory::append_message` / `load_active_conversation`
//! directly (no full Repl or AgentLoop) — the goal is to lock in that data
//! written in "session 1" survives closing and re-opening the SQLite file.

use llmsh_core::memory::{ConversationMessage, Memory};

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

/// Session 1 writes two messages; session 2 reopens the same file and asserts
/// both rows are reloaded intact by `load_active_conversation`.
#[test]
fn messages_survive_session_boundary() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("memory.db");

    // Session 1: open → write → drop.
    {
        let mem = Memory::open(&db_path).unwrap();
        mem.append_message(&ConversationMessage {
            id: 0,
            ts: now(),
            role: "user".into(),
            content: "je suis stéphane".into(),
            tool_call_id: None,
            name: None,
            tool_calls_json: None,
            insert_source: "turn".into(),
        })
        .unwrap();
        mem.append_message(&ConversationMessage {
            id: 0,
            ts: now(),
            role: "assistant".into(),
            content: "noted".into(),
            tool_call_id: None,
            name: None,
            tool_calls_json: None,
            insert_source: "turn".into(),
        })
        .unwrap();
    }

    // Session 2: reopen the same file.
    let mem = Memory::open(&db_path).unwrap();
    let msgs = mem.load_active_conversation().unwrap();

    assert_eq!(msgs.len(), 2, "both messages must persist across sessions");
    assert_eq!(msgs[0].role, "user");
    assert!(
        msgs[0].content.contains("stéphane"),
        "user message content must survive; got: {}",
        msgs[0].content
    );
    assert_eq!(msgs[1].role, "assistant");
    assert_eq!(msgs[1].content, "noted");
    assert_eq!(msgs[0].insert_source, "turn");
}

/// Verify that `tool_calls_json` (a JSON blob) round-trips intact across a
/// session boundary — critical for restoring tool-calling messages on startup.
#[test]
fn tool_calls_json_round_trips_across_sessions() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("memory.db");

    let tc_json = r#"[{"id":"tc1","name":"foo","arguments":"{}"}]"#;

    // Session 1: write an assistant message with tool_calls_json.
    {
        let mem = Memory::open(&db_path).unwrap();
        mem.append_message(&ConversationMessage {
            id: 0,
            ts: now(),
            role: "assistant".into(),
            content: String::new(),
            tool_call_id: None,
            name: None,
            tool_calls_json: Some(tc_json.into()),
            insert_source: "turn".into(),
        })
        .unwrap();
    }

    // Session 2: reload and verify the blob is unchanged.
    let mem = Memory::open(&db_path).unwrap();
    let msgs = mem.load_active_conversation().unwrap();

    assert_eq!(msgs.len(), 1);
    let got = msgs[0].tool_calls_json.as_deref().unwrap_or("");
    assert_eq!(
        got, tc_json,
        "tool_calls_json blob must survive a session boundary intact"
    );
}

/// Opening the same file multiple times in sequence must not corrupt the schema
/// (migration idempotency) and all previously written messages remain readable.
#[test]
fn multi_open_preserves_all_messages() {
    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("memory.db");

    // First open: write one message.
    {
        let mem = Memory::open(&db_path).unwrap();
        mem.append_message(&make_msg("user", "first")).unwrap();
    }

    // Second open: write another, then verify both are present.
    {
        let mem = Memory::open(&db_path).unwrap();
        mem.append_message(&make_msg("assistant", "second"))
            .unwrap();
        let msgs = mem.load_active_conversation().unwrap();
        assert_eq!(msgs.len(), 2);
    }

    // Third open: just read — still 2 rows.
    {
        let mem = Memory::open(&db_path).unwrap();
        let msgs = mem.load_active_conversation().unwrap();
        assert_eq!(
            msgs.len(),
            2,
            "third open must still see both rows without duplication"
        );
        assert_eq!(msgs[0].content, "first");
        assert_eq!(msgs[1].content, "second");
    }
}
