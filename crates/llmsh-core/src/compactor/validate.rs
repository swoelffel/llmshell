use llmsh_llm::types::{Message, MessageRole};
use std::collections::HashSet;

/// Verifies that the message sequence is valid for OpenAI's API:
/// every `tool` message has a preceding `assistant` with a matching tool_call id,
/// and every `assistant.tool_calls` entry has its `tool` response within the
/// remaining sequence.
///
/// Returns Ok(()) when the sequence is internally consistent.
pub fn validate_no_orphans(messages: &[Message]) -> Result<(), String> {
    let mut pending_ids: HashSet<String> = HashSet::new();
    let mut announced_ids: HashSet<String> = HashSet::new();

    for (idx, m) in messages.iter().enumerate() {
        match m.role {
            MessageRole::Assistant => {
                if let Some(tcs) = &m.tool_calls {
                    for tc in tcs {
                        announced_ids.insert(tc.id.clone());
                        pending_ids.insert(tc.id.clone());
                    }
                }
            }
            MessageRole::Tool => {
                let tc_id = m
                    .tool_call_id
                    .as_deref()
                    .ok_or_else(|| format!("tool message at index {} has no tool_call_id", idx))?;
                if !announced_ids.contains(tc_id) {
                    return Err(format!(
                        "tool message at index {} references tool_call_id {:?} not announced by any prior assistant",
                        idx, tc_id
                    ));
                }
                pending_ids.remove(tc_id);
            }
            _ => {}
        }
    }

    if !pending_ids.is_empty() {
        let mut ids: Vec<String> = pending_ids.into_iter().collect();
        ids.sort();
        return Err(format!(
            "assistant tool_calls without responses: {}",
            ids.join(", ")
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use llmsh_llm::types::{Message, MessageRole, ToolCall};
    use serde_json::json;

    fn user(s: &str) -> Message {
        Message {
            role: MessageRole::User,
            content: s.into(),
            tool_call_id: None,
            name: None,
            tool_calls: None,
        }
    }

    fn assistant_calls(content: &str, calls: Vec<(&str, &str)>) -> Message {
        Message {
            role: MessageRole::Assistant,
            content: content.into(),
            tool_call_id: None,
            name: None,
            tool_calls: Some(
                calls
                    .into_iter()
                    .map(|(id, name)| ToolCall {
                        id: id.into(),
                        name: name.into(),
                        args: json!({}),
                    })
                    .collect(),
            ),
        }
    }

    fn tool(id: &str, name: &str, content: &str) -> Message {
        Message {
            role: MessageRole::Tool,
            content: content.into(),
            tool_call_id: Some(id.into()),
            name: Some(name.into()),
            tool_calls: None,
        }
    }

    #[test]
    fn empty_is_valid() {
        assert!(validate_no_orphans(&[]).is_ok());
    }

    #[test]
    fn well_formed_pair_is_valid() {
        let seq = vec![
            user("hi"),
            assistant_calls("", vec![("c1", "list")]),
            tool("c1", "list", r#"{"status":"success"}"#),
        ];
        assert!(validate_no_orphans(&seq).is_ok());
    }

    #[test]
    fn orphan_tool_is_invalid() {
        let seq = vec![user("hi"), tool("ghost", "list", r#"{}"#)];
        assert!(validate_no_orphans(&seq).is_err());
    }

    #[test]
    fn unanswered_tool_call_is_invalid() {
        let seq = vec![
            user("hi"),
            assistant_calls("", vec![("c1", "list"), ("c2", "read")]),
            tool("c1", "list", r#"{}"#),
        ];
        assert!(validate_no_orphans(&seq).is_err());
    }
}
