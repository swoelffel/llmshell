use crate::wire::*;
use llmsh_llm::types::*;

pub fn to_wire_messages(system: Option<&str>, msgs: &[Message]) -> Vec<WireMessage> {
    let mut out = Vec::new();
    if let Some(sys) = system {
        out.push(WireMessage {
            role: "system".into(),
            content: Some(sys.to_string()),
            tool_call_id: None,
            name: None,
            tool_calls: None,
        });
    }
    for m in msgs {
        let role = match m.role {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::Tool => "tool",
        };
        // For assistant messages with tool_calls, the OpenAI wire format
        // expects content=null (or empty) and the tool_calls array populated.
        let wire_tool_calls = m.tool_calls.as_ref().map(|tcs| {
            tcs.iter()
                .map(|tc| WireToolCall {
                    id: tc.id.clone(),
                    kind: "function".into(),
                    function: WireFunctionCall {
                        name: tc.name.clone(),
                        arguments: serde_json::to_string(&tc.args).unwrap_or_else(|_| "{}".into()),
                    },
                })
                .collect()
        });
        let content = if wire_tool_calls.is_some() && m.content.is_empty() {
            None
        } else {
            Some(m.content.clone())
        };
        out.push(WireMessage {
            role: role.into(),
            content,
            tool_call_id: m.tool_call_id.clone(),
            name: m.name.clone(),
            tool_calls: wire_tool_calls,
        });
    }
    out
}

pub fn to_wire_tools(specs: &[ToolSpec]) -> Vec<WireTool> {
    specs
        .iter()
        .map(|s| WireTool {
            kind: "function",
            function: WireFunction {
                name: s.name.clone(),
                description: s.description.clone(),
                parameters: s.input_schema.clone(),
            },
        })
        .collect()
}

pub fn tool_choice_for(hint: ToolPolicyHint) -> Option<serde_json::Value> {
    match hint {
        ToolPolicyHint::None => Some(serde_json::json!("none")),
        ToolPolicyHint::PreferTools => Some(serde_json::json!("auto")),
        ToolPolicyHint::RequireTools => Some(serde_json::json!("required")),
    }
}

pub fn parse_response(resp: ChatResponse) -> anyhow::Result<LlmResponse> {
    let choice = resp
        .choices
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("no choices"))?;
    let finish = match choice.finish_reason.as_deref() {
        Some("stop") => FinishReason::Stop,
        Some("tool_calls") | Some("function_call") => FinishReason::ToolCalls,
        Some("length") => FinishReason::Length,
        Some("content_filter") => FinishReason::Refusal,
        _ => FinishReason::Stop,
    };
    let mut tool_calls = Vec::new();
    if let Some(tcs) = choice.message.tool_calls {
        for tc in tcs {
            let args: serde_json::Value = serde_json::from_str(&tc.function.arguments)
                .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));
            tool_calls.push(ToolCall {
                id: tc.id,
                name: tc.function.name,
                args,
            });
        }
    }
    let usage = resp.usage.map(|u| TokenUsage {
        input_tokens: u.prompt_tokens,
        output_tokens: u.completion_tokens,
        total_tokens: u.total_tokens,
        cached_input_tokens: u
            .prompt_tokens_details
            .as_ref()
            .and_then(|d| d.cached_tokens),
    });
    let final_finish = if !tool_calls.is_empty() {
        FinishReason::ToolCalls
    } else {
        finish
    };
    Ok(LlmResponse {
        message: choice.message.content,
        tool_calls,
        finish_reason: final_finish,
        usage,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_cached_tokens_from_prompt_details() {
        let raw = r#"{
            "choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"hi"}}],
            "usage":{
                "prompt_tokens":1234,
                "completion_tokens":56,
                "total_tokens":1290,
                "prompt_tokens_details":{"cached_tokens":900}
            }
        }"#;
        let parsed: ChatResponse = serde_json::from_str(raw).unwrap();
        let r = parse_response(parsed).unwrap();
        let u = r.usage.expect("usage present");
        assert_eq!(u.input_tokens, Some(1234));
        assert_eq!(u.output_tokens, Some(56));
        assert_eq!(u.cached_input_tokens, Some(900));
    }

    #[test]
    fn cached_tokens_absent_is_none() {
        let raw = r#"{
            "choices":[{"finish_reason":"stop","message":{"role":"assistant","content":"hi"}}],
            "usage":{"prompt_tokens":10,"completion_tokens":2,"total_tokens":12}
        }"#;
        let parsed: ChatResponse = serde_json::from_str(raw).unwrap();
        let r = parse_response(parsed).unwrap();
        assert_eq!(r.usage.unwrap().cached_input_tokens, None);
    }

    #[test]
    fn to_wire_messages_emits_assistant_with_tool_calls_before_tool() {
        use llmsh_llm::types::{Message, MessageRole, ToolCall};
        use serde_json::json;

        let messages = vec![
            Message {
                role: MessageRole::User,
                content: "list the dir".into(),
                tool_call_id: None,
                name: None,
                tool_calls: None,
            },
            Message {
                role: MessageRole::Assistant,
                content: "".into(),
                tool_call_id: None,
                name: None,
                tool_calls: Some(vec![ToolCall {
                    id: "call_abc".into(),
                    name: "list_directory".into(),
                    args: json!({"path": "."}),
                }]),
            },
            Message {
                role: MessageRole::Tool,
                content: r#"{"status":"success"}"#.into(),
                tool_call_id: Some("call_abc".into()),
                name: Some("list_directory".into()),
                tool_calls: None,
            },
        ];
        let wire = to_wire_messages(None, &messages);
        assert_eq!(wire.len(), 3);

        // Order: user, assistant (with tool_calls), tool.
        assert_eq!(wire[0].role, "user");

        assert_eq!(wire[1].role, "assistant");
        let tcs = wire[1]
            .tool_calls
            .as_ref()
            .expect("assistant must carry tool_calls");
        assert_eq!(tcs.len(), 1);
        assert_eq!(tcs[0].id, "call_abc");
        assert_eq!(tcs[0].kind, "function");
        assert_eq!(tcs[0].function.name, "list_directory");
        assert_eq!(tcs[0].function.arguments, r#"{"path":"."}"#);
        // Content for an assistant turn that ONLY emits tool_calls should be
        // serialized as null on the wire (skip_serializing_if = "Option::is_none").
        assert!(wire[1].content.is_none());

        assert_eq!(wire[2].role, "tool");
        assert_eq!(wire[2].tool_call_id.as_deref(), Some("call_abc"));
        assert!(wire[2].tool_calls.is_none());
    }

    #[test]
    fn parses_tool_call() {
        let resp = ChatResponse {
            choices: vec![Choice {
                finish_reason: Some("tool_calls".into()),
                message: WireMessage {
                    role: "assistant".into(),
                    content: None,
                    tool_call_id: None,
                    name: None,
                    tool_calls: Some(vec![WireToolCall {
                        id: "call_1".into(),
                        kind: "function".into(),
                        function: WireFunctionCall {
                            name: "list_directory".into(),
                            arguments: r#"{"path":"."}"#.into(),
                        },
                    }]),
                },
            }],
            usage: None,
        };
        let r = parse_response(resp).unwrap();
        assert_eq!(r.finish_reason, FinishReason::ToolCalls);
        assert_eq!(r.tool_calls.len(), 1);
        assert_eq!(r.tool_calls[0].id, "call_1");
        assert_eq!(r.tool_calls[0].args, json!({"path":"."}));
    }
}
