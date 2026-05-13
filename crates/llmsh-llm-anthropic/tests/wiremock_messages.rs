//! End-to-end provider tests against a wiremock-backed Anthropic Messages API.

use llmsh_llm::provider::LlmProvider;
use llmsh_llm::types::{
    FinishReason, LlmRequest, Message, MessageRole, ResponseFormat, ToolPolicyHint, ToolSpec,
};
use llmsh_llm_anthropic::provider::{AnthropicConfig, AnthropicProvider, DEFAULT_MAX_TOKENS};
use serde_json::{json, Value};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn provider(server: &MockServer, model: &str) -> AnthropicProvider {
    AnthropicProvider::new(AnthropicConfig {
        base_url: server.uri(),
        api_key: "sk-ant-api03-EXAMPLE_FIXTURE_NOT_REAL_aaaaaaaaaaaaaaa".into(),
        model: model.into(),
        timeout_ms: 5_000,
        max_tokens: DEFAULT_MAX_TOKENS,
    })
    .unwrap()
}

#[tokio::test]
async fn tool_use_round_trip() {
    let server = MockServer::start().await;

    // Turn 1: assistant requests a tool_use.
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "sk-ant-api03-EXAMPLE_FIXTURE_NOT_REAL_aaaaaaaaaaaaaaa"))
        .and(header("anthropic-version", "2023-06-01"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "content": [
                {"type": "text", "text": "I'll check."},
                {"type": "tool_use", "id": "toolu_01", "name": "list_directory", "input": {"path": "."}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 100, "output_tokens": 20, "cache_read_input_tokens": 30}
        })))
        .up_to_n_times(1)
        .mount(&server)
        .await;

    let p = provider(&server, "claude-haiku-4-5");

    let req1 = LlmRequest {
        system: Some("be brief".into()),
        messages: vec![Message {
            role: MessageRole::User,
            content: "list the dir".into(),
            tool_call_id: None,
            name: None,
            tool_calls: None,
        }],
        tools: vec![ToolSpec {
            name: "list_directory".into(),
            description: "list a directory".into(),
            input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}}}),
        }],
        tool_policy: ToolPolicyHint::PreferTools,
        response_format: None,
    };
    let r1 = p.complete(req1).await.unwrap();
    assert_eq!(r1.finish_reason, FinishReason::ToolCalls);
    assert_eq!(r1.tool_calls.len(), 1);
    assert_eq!(r1.tool_calls[0].id, "toolu_01");
    assert_eq!(r1.tool_calls[0].name, "list_directory");
    let u = r1.usage.unwrap();
    assert_eq!(u.cached_input_tokens, Some(30));
    // input_tokens neutral = uncached(100) + cached(30) = 130.
    assert_eq!(u.input_tokens, Some(130));

    // Turn 2: with tool_result, end_turn.
    server.reset().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "content": [{"type": "text", "text": "Done."}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 110, "output_tokens": 5}
        })))
        .mount(&server)
        .await;

    let req2 = LlmRequest {
        system: Some("be brief".into()),
        messages: vec![
            Message {
                role: MessageRole::User,
                content: "list the dir".into(),
                tool_call_id: None,
                name: None,
                tool_calls: None,
            },
            Message {
                role: MessageRole::Assistant,
                content: "I'll check.".into(),
                tool_call_id: None,
                name: None,
                tool_calls: Some(r1.tool_calls.clone()),
            },
            Message {
                role: MessageRole::Tool,
                content: r#"{"entries":["a","b"]}"#.into(),
                tool_call_id: Some("toolu_01".into()),
                name: Some("list_directory".into()),
                tool_calls: None,
            },
        ],
        tools: vec![ToolSpec {
            name: "list_directory".into(),
            description: "list a directory".into(),
            input_schema: json!({"type": "object"}),
        }],
        tool_policy: ToolPolicyHint::PreferTools,
        response_format: None,
    };
    let r2 = p.complete(req2).await.unwrap();
    assert_eq!(r2.finish_reason, FinishReason::Stop);
    assert_eq!(r2.message.as_deref(), Some("Done."));
}

#[tokio::test]
async fn body_shape_lifts_system_and_uses_tool_choice_any() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(move |req: &Request| {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            // Body assertions: system at top-level, max_tokens present,
            // tool_choice.type == "any" because hint = RequireTools.
            assert_eq!(body["system"], "you are a shell");
            assert!(body["max_tokens"].is_number());
            assert_eq!(body["tool_choice"]["type"], "any");
            assert_eq!(body["messages"][0]["role"], "user");
            assert_eq!(body["messages"][0]["content"][0]["type"], "text");
            ResponseTemplate::new(200).set_body_json(json!({
                "content": [{"type": "text", "text": "ok"}],
                "stop_reason": "end_turn"
            }))
        })
        .mount(&server)
        .await;

    let p = provider(&server, "claude-sonnet-4-6");
    let req = LlmRequest {
        system: Some("you are a shell".into()),
        messages: vec![Message {
            role: MessageRole::User,
            content: "hi".into(),
            tool_call_id: None,
            name: None,
            tool_calls: None,
        }],
        tools: vec![ToolSpec {
            name: "noop".into(),
            description: "".into(),
            input_schema: json!({"type": "object"}),
        }],
        tool_policy: ToolPolicyHint::RequireTools,
        response_format: None,
    };
    let r = p.complete(req).await.unwrap();
    assert_eq!(r.finish_reason, FinishReason::Stop);
}

#[tokio::test]
async fn json_prefill_round_trip_produces_parsable_json() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(move |req: &Request| {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            // Last message must be the prefill assistant `{`.
            let messages = body["messages"].as_array().unwrap();
            let last = messages.last().unwrap();
            assert_eq!(last["role"], "assistant");
            assert_eq!(last["content"][0]["type"], "text");
            assert_eq!(last["content"][0]["text"], "{");
            // No tools, no tool_choice.
            assert!(body.get("tools").is_none() || body["tools"].as_array().unwrap().is_empty());
            assert!(body.get("tool_choice").is_none());

            ResponseTemplate::new(200).set_body_json(json!({
                "content": [{
                    "type": "text",
                    "text": "\"summary\":\"hi\",\"facts\":[{\"category\":\"todo\",\"claim\":\"x\"}]}"
                }],
                "stop_reason": "end_turn"
            }))
        })
        .mount(&server)
        .await;

    let p = provider(&server, "claude-haiku-4-5");
    let req = LlmRequest {
        system: Some("emit json".into()),
        messages: vec![Message {
            role: MessageRole::User,
            content: "give json".into(),
            tool_call_id: None,
            name: None,
            tool_calls: None,
        }],
        tools: vec![],
        tool_policy: ToolPolicyHint::None,
        response_format: Some(ResponseFormat::JsonObject),
    };
    let r = p.complete(req).await.unwrap();
    let body = r.message.expect("message text");
    let v: Value = serde_json::from_str(&body).expect("must be valid JSON");
    assert_eq!(v["summary"], "hi");
    assert_eq!(v["facts"][0]["category"], "todo");
}

#[tokio::test]
async fn http_error_is_redacted_in_message() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(ResponseTemplate::new(401).set_body_string(
            r#"{"error":"bad key sk-ant-api03-LEAKY_FIXTURE_NOT_REAL_aaaaaaaaaaa"}"#,
        ))
        .mount(&server)
        .await;

    let p = provider(&server, "claude-haiku-4-5");
    let req = LlmRequest {
        system: None,
        messages: vec![Message {
            role: MessageRole::User,
            content: "x".into(),
            tool_call_id: None,
            name: None,
            tool_calls: None,
        }],
        tools: vec![],
        tool_policy: ToolPolicyHint::None,
        response_format: None,
    };
    let err = p.complete(req).await.unwrap_err();
    let msg = format!("{:#}", err);
    assert!(msg.contains("anthropic http 401"), "got: {msg}");
    assert!(
        !msg.contains("sk-ant-api03-LEAKY_FIXTURE_NOT_REAL_aaaaaaaaaaa"),
        "leaked key in error: {msg}"
    );
    assert!(msg.contains("[REDACTED:anthropic_key]"));
}
