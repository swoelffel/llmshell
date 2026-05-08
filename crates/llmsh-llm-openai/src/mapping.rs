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
        out.push(WireMessage {
            role: role.into(),
            content: Some(m.content.clone()),
            tool_call_id: m.tool_call_id.clone(),
            name: m.name.clone(),
            tool_calls: None,
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
