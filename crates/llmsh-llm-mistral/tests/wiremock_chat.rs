use llmsh_llm::provider::LlmProvider;
use llmsh_llm::types::{
    FinishReason, LlmRequest, Message, MessageRole, ResponseFormat, ToolPolicyHint, ToolSpec,
};
use llmsh_llm_mistral::provider::{MistralConfig, MistralProvider};
use serde_json::{json, Value};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

fn provider(server: &MockServer, model: &str) -> MistralProvider {
    MistralProvider::new(MistralConfig {
        base_url: format!("{}/v1", server.uri()),
        api_key: "mistral-api-key-EXAMPLE_FIXTURE_NOT_REAL".into(),
        model: model.into(),
        timeout_ms: 5_000,
    })
    .unwrap()
}

#[tokio::test]
async fn tool_call_round_trip_uses_chat_completions_shape() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(header(
            "authorization",
            "Bearer mistral-api-key-EXAMPLE_FIXTURE_NOT_REAL",
        ))
        .respond_with(move |req: &Request| {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            assert_eq!(body["model"], "mistral-medium-latest");
            assert_eq!(body["tool_choice"], "auto");
            assert_eq!(body["parallel_tool_calls"], true);
            assert_eq!(body["messages"][0]["role"], "system");
            assert_eq!(body["messages"][1]["role"], "user");
            assert_eq!(body["tools"][0]["type"], "function");
            assert_eq!(body["tools"][0]["function"]["name"], "list_directory");

            ResponseTemplate::new(200).set_body_json(json!({
                "id": "cmpl_1",
                "object": "chat.completion",
                "model": "mistral-medium-latest",
                "choices": [{
                    "index": 0,
                    "finish_reason": "tool_calls",
                    "message": {
                        "role": "assistant",
                        "content": "",
                        "tool_calls": [{
                            "id": "call_1",
                            "type": "function",
                            "function": {
                                "name": "list_directory",
                                "arguments": "{\"path\":\".\"}"
                            }
                        }]
                    }
                }],
                "usage": {
                    "prompt_tokens": 12,
                    "completion_tokens": 3,
                    "total_tokens": 15
                }
            }))
        })
        .mount(&server)
        .await;

    let p = provider(&server, "mistral-medium-latest");
    let req = LlmRequest {
        system: Some("be brief".into()),
        messages: vec![Message {
            role: MessageRole::User,
            content: "list files".into(),
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

    let out = p.complete(req).await.unwrap();
    assert_eq!(out.finish_reason, FinishReason::ToolCalls);
    assert_eq!(out.tool_calls.len(), 1);
    assert_eq!(out.tool_calls[0].id, "call_1");
    assert_eq!(out.tool_calls[0].name, "list_directory");
    assert_eq!(out.tool_calls[0].args, json!({"path": "."}));
    let usage = out.usage.unwrap();
    assert_eq!(usage.input_tokens, Some(12));
    assert_eq!(usage.output_tokens, Some(3));
    assert_eq!(usage.total_tokens, Some(15));
}

#[tokio::test]
async fn json_object_response_format_is_forwarded() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(move |req: &Request| {
            let body: Value = serde_json::from_slice(&req.body).unwrap();
            assert_eq!(body["response_format"]["type"], "json_object");
            assert!(body.get("tools").is_none());
            assert!(body.get("tool_choice").is_none());

            ResponseTemplate::new(200).set_body_json(json!({
                "choices": [{
                    "index": 0,
                    "finish_reason": "stop",
                    "message": {
                        "role": "assistant",
                        "content": "{\"summary\":\"ok\"}"
                    }
                }]
            }))
        })
        .mount(&server)
        .await;

    let p = provider(&server, "mistral-small-latest");
    let req = LlmRequest {
        system: Some("emit json".into()),
        messages: vec![Message {
            role: MessageRole::User,
            content: "summarize".into(),
            tool_call_id: None,
            name: None,
            tool_calls: None,
        }],
        tools: vec![],
        tool_policy: ToolPolicyHint::None,
        response_format: Some(ResponseFormat::JsonObject),
    };

    let out = p.complete(req).await.unwrap();
    assert_eq!(out.finish_reason, FinishReason::Stop);
    assert_eq!(out.message.as_deref(), Some("{\"summary\":\"ok\"}"));
}

#[tokio::test]
async fn list_models_returns_raw_models() {
    let server = MockServer::start().await;

    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header(
            "authorization",
            "Bearer mistral-api-key-EXAMPLE_FIXTURE_NOT_REAL",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "object": "list",
            "data": [
                {"id": "mistral-medium-latest", "owned_by": "mistralai", "created": 1770000000},
                {"id": "codestral-latest", "owned_by": "mistralai", "created": 1770000001}
            ]
        })))
        .mount(&server)
        .await;

    let p = provider(&server, "mistral-medium-latest");
    let models = p.list_models().await.unwrap();
    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, vec!["mistral-medium-latest", "codestral-latest"]);
    assert_eq!(models[0].owned_by.as_deref(), Some("mistralai"));
    assert_eq!(models[0].created, Some(1770000000));
}

#[tokio::test]
async fn http_error_is_redacted() {
    let server = MockServer::start().await;

    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(401).set_body_string(
                r#"{"error":"bad token mistral-api-key-EXAMPLE_FIXTURE_NOT_REAL"}"#,
            ),
        )
        .mount(&server)
        .await;

    let p = provider(&server, "mistral-small-latest");
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

    let err = p.complete(req).await.unwrap_err().to_string();
    assert!(err.contains("mistral http 401"));
    assert!(!err.contains("mistral-api-key-EXAMPLE_FIXTURE_NOT_REAL"));
}
