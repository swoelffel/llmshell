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
        let wire_tool_calls = m.tool_calls.as_ref().map(|tcs| {
            tcs.iter()
                .map(|tc| WireToolCall {
                    id: tc.id.clone(),
                    kind: "function".into(),
                    function: WireFunctionCall {
                        name: tc.name.clone(),
                        arguments: serde_json::to_value(&tc.args)
                            .unwrap_or_else(|_| serde_json::json!("{}")),
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
            let args = match tc.function.arguments {
                serde_json::Value::String(s) => serde_json::from_str(&s)
                    .unwrap_or_else(|_| serde_json::Value::Object(serde_json::Map::new())),
                other => other,
            };
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
        cached_input_tokens: None,
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
    fn parses_tool_arguments_when_returned_as_object() {
        let parsed: ChatResponse = serde_json::from_value(json!({
            "choices": [{
                "finish_reason": "tool_calls",
                "message": {
                    "role": "assistant",
                    "tool_calls": [{
                        "id": "call_1",
                        "type": "function",
                        "function": {
                            "name": "read_file",
                            "arguments": {"path": "Cargo.toml"}
                        }
                    }]
                }
            }]
        }))
        .unwrap();
        let out = parse_response(parsed).unwrap();
        assert_eq!(out.finish_reason, FinishReason::ToolCalls);
        assert_eq!(out.tool_calls[0].args, json!({"path": "Cargo.toml"}));
    }
}
