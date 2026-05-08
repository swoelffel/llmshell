use llmsh_llm::provider::LlmProvider;
use llmsh_llm_openai::provider::{OpenAIConfig, OpenAIProvider};
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn list_models_returns_all_models_unfiltered() {
    let server = MockServer::start().await;

    let fixture = serde_json::json!({
        "data": [
            { "id": "gpt-4o", "owned_by": "openai", "created": 1715367049 },
            { "id": "gpt-4o-mini", "owned_by": "openai", "created": 1715367050 },
            { "id": "whisper-1", "owned_by": "openai", "created": 1677532384 },
            { "id": "text-embedding-3-small", "owned_by": "openai", "created": 1705948997 },
            { "id": "o3-mini", "owned_by": "openai", "created": 1715367051 },
        ],
        "object": "list"
    });

    Mock::given(method("GET"))
        .and(path("/v1/models"))
        .and(header("authorization", "Bearer test-key"))
        .respond_with(ResponseTemplate::new(200).set_body_json(&fixture))
        .mount(&server)
        .await;

    let provider = OpenAIProvider::new(OpenAIConfig {
        base_url: format!("{}/v1", server.uri()),
        api_key: "test-key".into(),
        model: "gpt-4o-mini".into(),
        timeout_ms: 5000,
    })
    .unwrap();

    let models = provider.list_models().await.unwrap();
    let ids: Vec<&str> = models.iter().map(|m| m.id.as_str()).collect();

    assert!(ids.contains(&"gpt-4o"), "gpt-4o should be returned");
    assert!(
        ids.contains(&"gpt-4o-mini"),
        "gpt-4o-mini should be returned"
    );
    assert!(ids.contains(&"o3-mini"), "o3-mini should be returned");
    // All models from /v1/models are returned (filtering is done in model_cmd)
    assert!(ids.contains(&"whisper-1"), "whisper-1 is returned raw");
    assert_eq!(models.len(), 5);
}
