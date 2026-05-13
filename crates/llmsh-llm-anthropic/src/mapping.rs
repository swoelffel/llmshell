//! Neutral ↔ Anthropic Messages API translation.

use crate::wire::*;
use anyhow::anyhow;
use llmsh_llm::types::*;

/// Build the wire request from the neutral one.
///
/// Anthropic peculiarities handled here:
/// - `system` becomes a top-level string field.
/// - Tool calls live as `tool_use` blocks inside the `assistant` message that
///   emitted them.
/// - Tool responses are encoded as `tool_result` blocks in a `user` message.
///   Consecutive neutral `Tool` messages are merged into one `user` message.
/// - JSON mode (`response_format == JsonObject`) is emulated by appending an
///   assistant message prefilled with `"{"` (Anthropic cookbook).
pub fn to_wire<'a>(
    req: &LlmRequest,
    model: &'a str,
    max_tokens: u32,
) -> anyhow::Result<MessagesRequest<'a>> {
    let json_prefill = matches!(req.response_format, Some(ResponseFormat::JsonObject));
    if json_prefill && !req.tools.is_empty() {
        return Err(anyhow!(
            "anthropic provider cannot combine JSON-object response_format with tools"
        ));
    }

    let mut messages: Vec<WireMessage> = Vec::with_capacity(req.messages.len());

    for m in &req.messages {
        match m.role {
            MessageRole::System => {
                // Anthropic rejects role=system in messages; surface a clear
                // error so callers move the prompt to `req.system`.
                return Err(anyhow!(
                    "anthropic provider expects system prompts via LlmRequest.system, not as a Message"
                ));
            }
            MessageRole::User => {
                messages.push(WireMessage {
                    role: "user".into(),
                    content: vec![ContentBlock::Text {
                        text: m.content.clone(),
                    }],
                });
            }
            MessageRole::Assistant => {
                let mut blocks: Vec<ContentBlock> = Vec::new();
                if !m.content.is_empty() {
                    blocks.push(ContentBlock::Text {
                        text: m.content.clone(),
                    });
                }
                if let Some(tcs) = &m.tool_calls {
                    for tc in tcs {
                        blocks.push(ContentBlock::ToolUse {
                            id: tc.id.clone(),
                            name: tc.name.clone(),
                            input: tc.args.clone(),
                        });
                    }
                }
                if blocks.is_empty() {
                    // Edge: empty assistant turn (shouldn't happen but stay safe).
                    blocks.push(ContentBlock::Text {
                        text: String::new(),
                    });
                }
                messages.push(WireMessage {
                    role: "assistant".into(),
                    content: blocks,
                });
            }
            MessageRole::Tool => {
                let id = m.tool_call_id.clone().ok_or_else(|| {
                    anyhow!("tool message missing tool_call_id (Anthropic requires it)")
                })?;
                let block = ContentBlock::ToolResult {
                    tool_use_id: id,
                    content: m.content.clone(),
                };
                // Merge with the previous user message if it already carries
                // tool_result blocks (Anthropic recommends grouping consecutive
                // tool results into a single user turn).
                if let Some(last) = messages.last_mut() {
                    if last.role == "user"
                        && last
                            .content
                            .iter()
                            .all(|b| matches!(b, ContentBlock::ToolResult { .. }))
                    {
                        last.content.push(block);
                        continue;
                    }
                }
                messages.push(WireMessage {
                    role: "user".into(),
                    content: vec![block],
                });
            }
        }
    }

    if json_prefill {
        messages.push(WireMessage {
            role: "assistant".into(),
            content: vec![ContentBlock::Text { text: "{".into() }],
        });
    }

    Ok(MessagesRequest {
        model,
        max_tokens,
        system: req.system.clone(),
        messages,
        tools: req.tools.iter().map(to_wire_tool).collect(),
        tool_choice: tool_choice_for(req.tool_policy, req.tools.is_empty()),
    })
}

fn to_wire_tool(s: &ToolSpec) -> WireTool {
    WireTool {
        name: s.name.clone(),
        description: s.description.clone(),
        input_schema: s.input_schema.clone(),
    }
}

/// Map neutral hint to Anthropic `tool_choice`. Returns `None` when no tools
/// are advertised — Anthropic 400s on `tool_choice` without `tools`.
pub fn tool_choice_for(hint: ToolPolicyHint, no_tools: bool) -> Option<WireToolChoice> {
    if no_tools {
        return None;
    }
    Some(match hint {
        ToolPolicyHint::None => WireToolChoice::None,
        ToolPolicyHint::PreferTools => WireToolChoice::Auto,
        ToolPolicyHint::RequireTools => WireToolChoice::Any,
    })
}

/// Parse the wire response into the neutral one.
///
/// `json_prefill` must be `true` iff the matching request was sent with the
/// JSON-object prefill trick — the closing brace is then reconstructed by
/// taking the substring up to the last `}` in the assistant text and
/// prepending the `{` Anthropic never re-emits.
pub fn parse_response(resp: MessagesResponse, json_prefill: bool) -> anyhow::Result<LlmResponse> {
    let mut text = String::new();
    let mut tool_calls: Vec<ToolCall> = Vec::new();

    for block in resp.content {
        match block {
            ContentBlock::Text { text: t } => {
                text.push_str(&t);
            }
            ContentBlock::ToolUse { id, name, input } => {
                tool_calls.push(ToolCall {
                    id,
                    name,
                    args: input,
                });
            }
            ContentBlock::ToolResult { .. } => {
                // Anthropic does not emit tool_result on the response path;
                // ignore defensively.
            }
        }
    }

    if json_prefill {
        let trimmed = text.trim();
        let last_brace = trimmed.rfind('}').ok_or_else(|| {
            anyhow!("anthropic JSON prefill response missing closing '}}': {trimmed}")
        })?;
        text = format!("{{{}", &trimmed[..=last_brace]);
    }

    let finish = match resp.stop_reason.as_deref() {
        Some("tool_use") => FinishReason::ToolCalls,
        Some("end_turn") | Some("stop_sequence") => FinishReason::Stop,
        Some("max_tokens") => FinishReason::Length,
        Some("refusal") => FinishReason::Refusal,
        _ => FinishReason::Stop,
    };
    let final_finish = if !tool_calls.is_empty() {
        FinishReason::ToolCalls
    } else {
        finish
    };

    let usage = resp.usage.map(|u| {
        let cached = u.cache_read_input_tokens;
        let input = u.input_tokens.map(|i| i + cached.unwrap_or(0));
        TokenUsage {
            input_tokens: input,
            output_tokens: u.output_tokens,
            total_tokens: match (input, u.output_tokens) {
                (Some(i), Some(o)) => Some(i + o),
                _ => None,
            },
            cached_input_tokens: cached,
        }
    });

    let message = if text.is_empty() { None } else { Some(text) };
    Ok(LlmResponse {
        message,
        tool_calls,
        finish_reason: final_finish,
        usage,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn user(text: &str) -> Message {
        Message {
            role: MessageRole::User,
            content: text.into(),
            tool_call_id: None,
            name: None,
            tool_calls: None,
        }
    }

    fn assistant_tool_call(text: &str, id: &str, name: &str, args: serde_json::Value) -> Message {
        Message {
            role: MessageRole::Assistant,
            content: text.into(),
            tool_call_id: None,
            name: None,
            tool_calls: Some(vec![ToolCall {
                id: id.into(),
                name: name.into(),
                args,
            }]),
        }
    }

    fn tool_msg(id: &str, body: &str) -> Message {
        Message {
            role: MessageRole::Tool,
            content: body.into(),
            tool_call_id: Some(id.into()),
            name: None,
            tool_calls: None,
        }
    }

    fn base_req(messages: Vec<Message>) -> LlmRequest {
        LlmRequest {
            system: None,
            messages,
            tools: vec![],
            tool_policy: ToolPolicyHint::PreferTools,
            response_format: None,
        }
    }

    #[test]
    fn system_lifted_to_top_level() {
        let mut req = base_req(vec![user("hi")]);
        req.system = Some("be brief".into());
        let wire = to_wire(&req, "claude-haiku-4-5", 4096).unwrap();
        assert_eq!(wire.system.as_deref(), Some("be brief"));
        assert_eq!(wire.messages.len(), 1);
        assert_eq!(wire.messages[0].role, "user");
    }

    #[test]
    fn system_absent_when_none() {
        let req = base_req(vec![user("hi")]);
        let wire = to_wire(&req, "claude-haiku-4-5", 4096).unwrap();
        assert!(wire.system.is_none());
    }

    #[test]
    fn system_role_in_messages_is_rejected() {
        let req = LlmRequest {
            system: None,
            messages: vec![Message {
                role: MessageRole::System,
                content: "x".into(),
                tool_call_id: None,
                name: None,
                tool_calls: None,
            }],
            tools: vec![],
            tool_policy: ToolPolicyHint::None,
            response_format: None,
        };
        assert!(to_wire(&req, "m", 1).is_err());
    }

    #[test]
    fn assistant_with_tool_call_becomes_tool_use_block() {
        let req = base_req(vec![
            user("list it"),
            assistant_tool_call("", "toolu_1", "list_directory", json!({"path": "."})),
            tool_msg("toolu_1", r#"{"status":"success"}"#),
        ]);
        let wire = to_wire(&req, "claude-haiku-4-5", 4096).unwrap();
        assert_eq!(wire.messages.len(), 3);

        assert_eq!(wire.messages[1].role, "assistant");
        match &wire.messages[1].content[0] {
            ContentBlock::ToolUse { id, name, input } => {
                assert_eq!(id, "toolu_1");
                assert_eq!(name, "list_directory");
                assert_eq!(input, &json!({"path": "."}));
            }
            other => panic!("expected ToolUse, got {:?}", other),
        }

        assert_eq!(wire.messages[2].role, "user");
        match &wire.messages[2].content[0] {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
            } => {
                assert_eq!(tool_use_id, "toolu_1");
                assert_eq!(content, r#"{"status":"success"}"#);
            }
            other => panic!("expected ToolResult, got {:?}", other),
        }
    }

    #[test]
    fn assistant_with_text_and_tool_call_emits_both_blocks() {
        let req = base_req(vec![
            user("list it"),
            assistant_tool_call(
                "Listing now.",
                "toolu_1",
                "list_directory",
                json!({"path": "."}),
            ),
        ]);
        let wire = to_wire(&req, "m", 1024).unwrap();
        let blocks = &wire.messages[1].content;
        assert_eq!(blocks.len(), 2);
        assert!(matches!(blocks[0], ContentBlock::Text { .. }));
        assert!(matches!(blocks[1], ContentBlock::ToolUse { .. }));
    }

    #[test]
    fn consecutive_tool_messages_merge_into_one_user_turn() {
        let req = base_req(vec![
            user("do two things"),
            Message {
                role: MessageRole::Assistant,
                content: "".into(),
                tool_call_id: None,
                name: None,
                tool_calls: Some(vec![
                    ToolCall {
                        id: "toolu_a".into(),
                        name: "t1".into(),
                        args: json!({}),
                    },
                    ToolCall {
                        id: "toolu_b".into(),
                        name: "t2".into(),
                        args: json!({}),
                    },
                ]),
            },
            tool_msg("toolu_a", "ra"),
            tool_msg("toolu_b", "rb"),
        ]);
        let wire = to_wire(&req, "m", 1024).unwrap();
        // user, assistant(2x tool_use), user(2x tool_result).
        assert_eq!(wire.messages.len(), 3);
        assert_eq!(wire.messages[2].role, "user");
        assert_eq!(wire.messages[2].content.len(), 2);
    }

    #[test]
    fn tool_choice_dropped_when_no_tools() {
        let req = base_req(vec![user("hi")]);
        let wire = to_wire(&req, "m", 1).unwrap();
        assert!(wire.tool_choice.is_none());
    }

    #[test]
    fn tool_choice_maps_each_hint() {
        assert_eq!(
            tool_choice_for(ToolPolicyHint::None, false),
            Some(WireToolChoice::None)
        );
        assert_eq!(
            tool_choice_for(ToolPolicyHint::PreferTools, false),
            Some(WireToolChoice::Auto)
        );
        assert_eq!(
            tool_choice_for(ToolPolicyHint::RequireTools, false),
            Some(WireToolChoice::Any)
        );
        assert_eq!(tool_choice_for(ToolPolicyHint::RequireTools, true), None);
    }

    #[test]
    fn json_prefill_appends_assistant_open_brace_when_requested() {
        let req = LlmRequest {
            system: None,
            messages: vec![user("give me JSON")],
            tools: vec![],
            tool_policy: ToolPolicyHint::None,
            response_format: Some(ResponseFormat::JsonObject),
        };
        let wire = to_wire(&req, "m", 1024).unwrap();
        assert_eq!(wire.messages.len(), 2);
        let last = wire.messages.last().unwrap();
        assert_eq!(last.role, "assistant");
        match &last.content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "{"),
            other => panic!("expected text {{, got {:?}", other),
        }
    }

    #[test]
    fn json_prefill_plus_tools_is_an_error() {
        let req = LlmRequest {
            system: None,
            messages: vec![user("x")],
            tools: vec![ToolSpec {
                name: "t".into(),
                description: "d".into(),
                input_schema: json!({}),
            }],
            tool_policy: ToolPolicyHint::PreferTools,
            response_format: Some(ResponseFormat::JsonObject),
        };
        assert!(to_wire(&req, "m", 1).is_err());
    }

    #[test]
    fn parse_response_concatenates_text_and_collects_tool_uses() {
        let resp = MessagesResponse {
            content: vec![
                ContentBlock::Text {
                    text: "Doing it.".into(),
                },
                ContentBlock::ToolUse {
                    id: "toolu_1".into(),
                    name: "list_directory".into(),
                    input: json!({"path":"."}),
                },
            ],
            stop_reason: Some("tool_use".into()),
            usage: None,
        };
        let r = parse_response(resp, false).unwrap();
        assert_eq!(r.finish_reason, FinishReason::ToolCalls);
        assert_eq!(r.tool_calls.len(), 1);
        assert_eq!(r.tool_calls[0].id, "toolu_1");
        assert_eq!(r.message.as_deref(), Some("Doing it."));
    }

    #[test]
    fn parse_response_reconstructs_json_when_prefill() {
        let resp = MessagesResponse {
            content: vec![ContentBlock::Text {
                text: "\"summary\":\"hi\",\"facts\":[]}\n\nignored trailing".into(),
            }],
            stop_reason: Some("end_turn".into()),
            usage: None,
        };
        let r = parse_response(resp, true).unwrap();
        let body = r.message.unwrap();
        // Must be valid JSON.
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["summary"], "hi");
        assert!(v["facts"].is_array());
    }

    #[test]
    fn parse_response_json_prefill_without_closing_brace_errors() {
        let resp = MessagesResponse {
            content: vec![ContentBlock::Text {
                text: "\"summary\":\"oops\"".into(),
            }],
            stop_reason: Some("max_tokens".into()),
            usage: None,
        };
        assert!(parse_response(resp, true).is_err());
    }

    #[test]
    fn parse_response_usage_includes_cache_read_in_input_tokens() {
        let resp = MessagesResponse {
            content: vec![ContentBlock::Text { text: "ok".into() }],
            stop_reason: Some("end_turn".into()),
            usage: Some(Usage {
                input_tokens: Some(100),
                output_tokens: Some(20),
                cache_read_input_tokens: Some(50),
                cache_creation_input_tokens: None,
            }),
        };
        let r = parse_response(resp, false).unwrap();
        let u = r.usage.unwrap();
        assert_eq!(u.input_tokens, Some(150));
        assert_eq!(u.cached_input_tokens, Some(50));
        assert_eq!(u.output_tokens, Some(20));
        assert_eq!(u.total_tokens, Some(170));
    }

    #[test]
    fn parse_response_stop_reason_mapping() {
        let mk = |sr: &str| MessagesResponse {
            content: vec![ContentBlock::Text { text: "x".into() }],
            stop_reason: Some(sr.into()),
            usage: None,
        };
        assert_eq!(
            parse_response(mk("end_turn"), false).unwrap().finish_reason,
            FinishReason::Stop
        );
        assert_eq!(
            parse_response(mk("max_tokens"), false)
                .unwrap()
                .finish_reason,
            FinishReason::Length
        );
        assert_eq!(
            parse_response(mk("refusal"), false).unwrap().finish_reason,
            FinishReason::Refusal
        );
        assert_eq!(
            parse_response(mk("stop_sequence"), false)
                .unwrap()
                .finish_reason,
            FinishReason::Stop
        );
    }
}
