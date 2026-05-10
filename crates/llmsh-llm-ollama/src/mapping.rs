use crate::wire::*;
use llmsh_llm::types::*;

pub fn to_wire_messages(system: Option<&str>, msgs: &[Message]) -> Vec<WireMessage> {
    let mut out = Vec::new();
    if let Some(sys) = system {
        out.push(WireMessage {
            role: "system".into(),
            content: sys.to_string(),
            tool_calls: None,
            name: None,
        });
    }
    for m in msgs {
        let role = match m.role {
            MessageRole::System => "system",
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::Tool => "tool",
        };
        let wire_tool_calls = m.tool_calls.as_ref().map(|tcs| {
            tcs.iter()
                .map(|tc| WireToolCall {
                    function: WireFunctionCall {
                        name: tc.name.clone(),
                        arguments: tc.args.clone(),
                    },
                })
                .collect()
        });
        out.push(WireMessage {
            role: role.into(),
            content: m.content.clone(),
            tool_calls: wire_tool_calls,
            name: m.name.clone(),
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

/// Synthesize a tool_call id (Ollama does not provide one).
fn synth_id(idx: usize, name: &str) -> String {
    format!("ollama-{}-{}", idx, name)
}

pub fn parse_response(resp: ChatResponse) -> anyhow::Result<LlmResponse> {
    let mut tool_calls = Vec::new();
    if let Some(tcs) = resp.message.tool_calls {
        for (i, tc) in tcs.into_iter().enumerate() {
            let id = synth_id(i, &tc.function.name);
            tool_calls.push(ToolCall {
                id,
                name: tc.function.name,
                args: tc.function.arguments,
            });
        }
    }
    let finish = if !tool_calls.is_empty() {
        FinishReason::ToolCalls
    } else {
        match resp.done_reason.as_deref() {
            Some("stop") | Some("end") | None => FinishReason::Stop,
            Some("length") => FinishReason::Length,
            _ => FinishReason::Stop,
        }
    };
    let usage = if resp.prompt_eval_count.is_some() || resp.eval_count.is_some() {
        Some(TokenUsage {
            input_tokens: resp.prompt_eval_count,
            output_tokens: resp.eval_count,
            total_tokens: match (resp.prompt_eval_count, resp.eval_count) {
                (Some(a), Some(b)) => Some(a + b),
                _ => None,
            },
            cached_input_tokens: None,
        })
    } else {
        None
    };
    let message = if resp.message.content.is_empty() {
        None
    } else {
        Some(resp.message.content)
    };
    Ok(LlmResponse {
        message,
        tool_calls,
        finish_reason: finish,
        usage,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn maps_assistant_with_tool_calls_to_wire() {
        let messages = vec![
            Message {
                role: MessageRole::User,
                content: "list".into(),
                tool_call_id: None,
                name: None,
                tool_calls: None,
            },
            Message {
                role: MessageRole::Assistant,
                content: String::new(),
                tool_call_id: None,
                name: None,
                tool_calls: Some(vec![ToolCall {
                    id: "ollama-0-list_directory".into(),
                    name: "list_directory".into(),
                    args: json!({"path": "."}),
                }]),
            },
            Message {
                role: MessageRole::Tool,
                content: r#"{"status":"ok"}"#.into(),
                tool_call_id: None,
                name: Some("list_directory".into()),
                tool_calls: None,
            },
        ];
        let wire = to_wire_messages(Some("be helpful"), &messages);
        assert_eq!(wire.len(), 4);
        assert_eq!(wire[0].role, "system");
        assert_eq!(wire[1].role, "user");
        assert_eq!(wire[2].role, "assistant");
        let tcs = wire[2].tool_calls.as_ref().unwrap();
        assert_eq!(tcs[0].function.name, "list_directory");
        assert_eq!(tcs[0].function.arguments, json!({"path":"."}));
        assert_eq!(wire[3].role, "tool");
        assert_eq!(wire[3].name.as_deref(), Some("list_directory"));
    }

    #[test]
    fn parses_response_with_tool_calls() {
        let raw = r#"{
            "model":"llama3.1",
            "message":{
                "role":"assistant",
                "content":"",
                "tool_calls":[
                    {"function":{"name":"list_directory","arguments":{"path":"."}}}
                ]
            },
            "done_reason":"stop",
            "prompt_eval_count":42,
            "eval_count":7
        }"#;
        let parsed: ChatResponse = serde_json::from_str(raw).unwrap();
        let r = parse_response(parsed).unwrap();
        assert_eq!(r.finish_reason, FinishReason::ToolCalls);
        assert_eq!(r.tool_calls.len(), 1);
        assert_eq!(r.tool_calls[0].name, "list_directory");
        assert_eq!(r.tool_calls[0].args, json!({"path":"."}));
        assert!(r.tool_calls[0].id.starts_with("ollama-0-"));
        let u = r.usage.unwrap();
        assert_eq!(u.input_tokens, Some(42));
        assert_eq!(u.output_tokens, Some(7));
        assert_eq!(u.total_tokens, Some(49));
    }

    #[test]
    fn parses_plain_text_response() {
        let raw = r#"{
            "model":"llama3.1",
            "message":{"role":"assistant","content":"hello"},
            "done_reason":"stop"
        }"#;
        let parsed: ChatResponse = serde_json::from_str(raw).unwrap();
        let r = parse_response(parsed).unwrap();
        assert_eq!(r.finish_reason, FinishReason::Stop);
        assert_eq!(r.message.as_deref(), Some("hello"));
        assert!(r.tool_calls.is_empty());
        assert!(r.usage.is_none());
    }

    #[test]
    fn empty_content_becomes_none() {
        let raw = r#"{"message":{"role":"assistant","content":""},"done_reason":"stop"}"#;
        let parsed: ChatResponse = serde_json::from_str(raw).unwrap();
        let r = parse_response(parsed).unwrap();
        assert!(r.message.is_none());
    }
}
