//! Verify the one-shot cleanup of orphan assistant.tool_calls rows.

use llmsh_core::memory::{ConversationMessage, Memory};

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn make_msg(
    role: &str,
    content: &str,
    tool_call_id: Option<&str>,
    tool_calls_json: Option<&str>,
) -> ConversationMessage {
    ConversationMessage {
        id: 0,
        ts: now(),
        role: role.into(),
        content: content.into(),
        tool_call_id: tool_call_id.map(String::from),
        name: None,
        tool_calls_json: tool_calls_json.map(String::from),
        insert_source: "turn".into(),
    }
}

#[test]
fn orphan_assistant_with_unmatched_tool_call_is_cleared() {
    let m = Memory::open_in_memory().unwrap();
    // user
    m.append_message(&make_msg("user", "go", None, None))
        .unwrap();
    // assistant with a tool_call that has no follow-up
    let tcs = r#"[{"id":"call_x","name":"read_file","args":{}}]"#;
    m.append_message(&make_msg("assistant", "", None, Some(tcs)))
        .unwrap();
    // user message after, no tool reply
    m.append_message(&make_msg("user", "next", None, None))
        .unwrap();

    let cleared = m.cleanup_orphan_tool_calls(&now()).unwrap();
    assert_eq!(cleared, 1, "orphan assistant row should be cleared");

    let active = m.load_active_conversation().unwrap();
    assert_eq!(active.len(), 2);
    assert!(
        !active
            .iter()
            .any(|r| r.role == "assistant" && r.tool_calls_json.is_some()),
        "assistant tool_calls row must be gone"
    );
}

#[test]
fn well_formed_tool_call_sequence_is_preserved() {
    let m = Memory::open_in_memory().unwrap();
    m.append_message(&make_msg("user", "go", None, None))
        .unwrap();
    let tcs = r#"[{"id":"call_y","name":"read_file","args":{}}]"#;
    m.append_message(&make_msg("assistant", "", None, Some(tcs)))
        .unwrap();
    m.append_message(&make_msg(
        "tool",
        r#"{"status":"success"}"#,
        Some("call_y"),
        None,
    ))
    .unwrap();
    m.append_message(&make_msg("assistant", "ok", None, None))
        .unwrap();

    let cleared = m.cleanup_orphan_tool_calls(&now()).unwrap();
    assert_eq!(cleared, 0, "well-formed sequence should not be cleared");
    let active = m.load_active_conversation().unwrap();
    assert_eq!(active.len(), 4);
}
