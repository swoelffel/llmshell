use llmsh_llm::types::{Message, MessageRole};
use serde_json::Value;

/// Truncate the `stdout` field of any `tool`-role message whose `content`
/// (full JSON) exceeds `max_bytes`. Returns the number of messages mutated.
/// Idempotent: a second call with the same `max_bytes` is a no-op.
pub fn truncate_tool_outputs(messages: &mut [Message], max_bytes: usize) -> usize {
    let mut changed = 0usize;
    for m in messages.iter_mut() {
        if m.role != MessageRole::Tool {
            continue;
        }
        if m.content.len() <= max_bytes {
            continue;
        }
        // Try to parse the JSON envelope produced by ContextBuilder.
        let mut json: Value = match serde_json::from_str(&m.content) {
            Ok(v) => v,
            Err(_) => continue, // unrecognized shape — leave alone
        };
        let Some(stdout_val) = json.get("stdout").cloned() else {
            continue;
        };
        let Some(stdout_str) = stdout_val.as_str() else {
            continue;
        };

        // Compute how many stdout bytes we can keep so that the re-serialized
        // JSON fits under `max_bytes`. Be conservative: target half of
        // `max_bytes` for stdout, leaving room for the rest of the envelope
        // and the truncation marker.
        let target_stdout = max_bytes.saturating_sub(256).max(64).min(max_bytes / 2);
        if stdout_str.len() <= target_stdout {
            continue;
        }
        let original_len = stdout_str.len();
        let cut: String = stdout_str.chars().take(target_stdout).collect();
        let truncated = format!("{}… (truncated, {} bytes original)", cut, original_len);
        json["stdout"] = Value::String(truncated);
        if let Some(obj) = json.as_object_mut() {
            obj.insert("truncated".into(), Value::Bool(true));
        }
        let new_content = match serde_json::to_string(&json) {
            Ok(s) if s.len() < m.content.len() => s,
            _ => continue,
        };
        m.content = new_content;
        changed += 1;
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use llmsh_llm::types::Message;

    fn tool_msg(stdout: &str) -> Message {
        let json = serde_json::json!({
            "status": "success",
            "stdout": stdout,
            "exit_code": 0,
            "truncated": false,
        });
        Message {
            role: MessageRole::Tool,
            content: json.to_string(),
            tool_call_id: Some("c1".into()),
            name: Some("list".into()),
            tool_calls: None,
        }
    }

    #[test]
    fn small_stdout_unchanged() {
        let mut msgs = vec![tool_msg("hello")];
        let changed = truncate_tool_outputs(&mut msgs, 2048);
        assert_eq!(changed, 0);
        assert!(msgs[0].content.contains("\"hello\""));
    }

    #[test]
    fn large_stdout_gets_truncated_marker() {
        let big = "x".repeat(8192);
        let mut msgs = vec![tool_msg(&big)];
        let changed = truncate_tool_outputs(&mut msgs, 1024);
        assert_eq!(changed, 1);
        assert!(msgs[0].content.contains("(truncated, 8192 bytes original)"));
        assert!(msgs[0].content.len() < 1500); // well under bound + envelope
                                               // truncated flag flipped
        assert!(msgs[0].content.contains("\"truncated\":true"));
    }

    #[test]
    fn idempotent_at_same_budget() {
        let big = "y".repeat(8192);
        let mut msgs = vec![tool_msg(&big)];
        truncate_tool_outputs(&mut msgs, 1024);
        let after_first = msgs[0].content.clone();
        let changed = truncate_tool_outputs(&mut msgs, 1024);
        assert_eq!(changed, 0, "second pass must be a no-op");
        assert_eq!(msgs[0].content, after_first);
    }

    #[test]
    fn non_tool_messages_untouched() {
        let mut msgs = vec![Message {
            role: MessageRole::User,
            content: "x".repeat(8192),
            tool_call_id: None,
            name: None,
            tool_calls: None,
        }];
        let original = msgs[0].content.clone();
        let changed = truncate_tool_outputs(&mut msgs, 100);
        assert_eq!(changed, 0);
        assert_eq!(msgs[0].content, original);
    }

    #[test]
    fn unparseable_tool_content_skipped() {
        let mut msgs = vec![Message {
            role: MessageRole::Tool,
            content: "not json at all - just some legacy free text".into(),
            tool_call_id: Some("c1".into()),
            name: Some("legacy".into()),
            tool_calls: None,
        }];
        let original = msgs[0].content.clone();
        let changed = truncate_tool_outputs(&mut msgs, 10);
        assert_eq!(changed, 0);
        assert_eq!(msgs[0].content, original);
    }
}
