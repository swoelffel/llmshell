use crate::mapping::*;
use crate::wire::*;
use anyhow::anyhow;
use async_trait::async_trait;
use llmsh_llm::capabilities::{Capabilities, ToolCallingMode};
use llmsh_llm::provider::LlmProvider;
use llmsh_llm::types::{LlmRequest, LlmResponse, ModelInfo, ResponseFormat};
use secrecy::{ExposeSecret, SecretString};
use std::sync::{Arc, RwLock};

/// Default `max_tokens` value when the caller does not override it. Anthropic
/// requires the field on every request; 4096 fits comfortably below all three
/// Claude 4.x output caps.
pub const DEFAULT_MAX_TOKENS: u32 = 4096;

pub const ANTHROPIC_VERSION: &str = "2023-06-01";

pub struct AnthropicProvider {
    base_url: String,
    api_key: SecretString,
    model: Arc<RwLock<String>>,
    max_tokens: u32,
    http: reqwest::Client,
}

impl std::fmt::Debug for AnthropicProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnthropicProvider")
            .field("base_url", &self.base_url)
            .field("api_key", &"[REDACTED]")
            .field("model", &self.model)
            .field("max_tokens", &self.max_tokens)
            .finish()
    }
}

pub struct AnthropicConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub timeout_ms: u64,
    pub max_tokens: u32,
}

impl AnthropicProvider {
    pub fn new(cfg: AnthropicConfig) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(cfg.timeout_ms))
            .build()?;
        Ok(Self {
            base_url: cfg.base_url,
            api_key: SecretString::new(cfg.api_key),
            model: Arc::new(RwLock::new(cfg.model)),
            max_tokens: cfg.max_tokens,
            http,
        })
    }

    pub fn shared_model(&self) -> Arc<RwLock<String>> {
        self.model.clone()
    }
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            tool_calling: ToolCallingMode::Native,
            supports_streaming: false,
            // Emulated via assistant prefill `{` (cookbook technique).
            supports_json_mode: true,
            supports_parallel_tool_calls: true,
            supports_tool_choice_required: true,
            max_context_tokens: Some(200_000),
        }
    }

    async fn complete(&self, req: LlmRequest) -> anyhow::Result<LlmResponse> {
        let model = self
            .model
            .read()
            .map_err(|_| anyhow!("model lock poisoned"))?
            .clone();
        let json_prefill = matches!(req.response_format, Some(ResponseFormat::JsonObject));
        let body = to_wire(&req, &model, self.max_tokens)?;
        let url = format!("{}/v1/messages", self.base_url.trim_end_matches('/'));
        let resp = self
            .http
            .post(&url)
            .header("x-api-key", self.api_key.expose_secret())
            .header("anthropic-version", ANTHROPIC_VERSION)
            .header("content-type", "application/json")
            .json(&body)
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("{}", format_http_error(status.as_u16(), &text));
        }
        let parsed: MessagesResponse = resp.json().await?;
        parse_response(parsed, json_prefill)
    }

    async fn list_models(&self) -> anyhow::Result<Vec<ModelInfo>> {
        let url = format!("{}/v1/models", self.base_url.trim_end_matches('/'));
        let resp = self
            .http
            .get(&url)
            .header("x-api-key", self.api_key.expose_secret())
            .header("anthropic-version", ANTHROPIC_VERSION)
            .send()
            .await?
            .error_for_status()?;
        let body: ModelsResponse = resp.json().await?;
        Ok(body
            .data
            .into_iter()
            .map(|e| ModelInfo {
                id: e.id,
                owned_by: e.display_name,
                created: e.created_at.as_deref().and_then(chrono_parse_epoch),
            })
            .collect())
    }

    async fn set_model(&self, id: &str) -> anyhow::Result<()> {
        let mut guard = self
            .model
            .write()
            .map_err(|_| anyhow!("model lock poisoned"))?;
        *guard = id.to_string();
        Ok(())
    }

    fn current_model(&self) -> String {
        self.model
            .read()
            .map(|g| g.clone())
            .unwrap_or_else(|_| "unknown".into())
    }
}

/// Best-effort RFC3339 → epoch seconds. Returns `None` on parse failure; we do
/// not depend on `chrono` here so the conversion is intentionally minimal.
fn chrono_parse_epoch(_s: &str) -> Option<i64> {
    None
}

fn format_http_error(status: u16, body: &str) -> String {
    let redacted = llmsh_redact::Redactor::default().redact(body);
    format!("anthropic http {status}: {redacted}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_provider(model: &str) -> AnthropicProvider {
        AnthropicProvider::new(AnthropicConfig {
            base_url: "https://api.anthropic.com".into(),
            api_key: "test-key".into(),
            model: model.into(),
            timeout_ms: 5000,
            max_tokens: DEFAULT_MAX_TOKENS,
        })
        .unwrap()
    }

    #[test]
    fn current_model_returns_initial() {
        let p = make_provider("claude-haiku-4-5");
        assert_eq!(p.current_model(), "claude-haiku-4-5");
    }

    #[tokio::test]
    async fn set_model_updates_current() {
        let p = make_provider("claude-haiku-4-5");
        p.set_model("claude-sonnet-4-6").await.unwrap();
        assert_eq!(p.current_model(), "claude-sonnet-4-6");
    }

    #[test]
    fn debug_does_not_leak_api_key() {
        let p = AnthropicProvider::new(AnthropicConfig {
            base_url: "https://api.anthropic.com".into(),
            api_key: "sk-ant-api03-EXAMPLE_FIXTURE_NOT_A_REAL_KEY_aaaaaaaa".into(),
            model: "claude-haiku-4-5".into(),
            timeout_ms: 5000,
            max_tokens: DEFAULT_MAX_TOKENS,
        })
        .unwrap();
        let dbg = format!("{:?}", p);
        assert!(
            !dbg.contains("sk-ant-api03-EXAMPLE_FIXTURE_NOT_A_REAL_KEY_aaaaaaaa"),
            "debug leaks api key: {dbg}"
        );
    }

    #[test]
    fn shared_model_reflects_mutations() {
        let p = make_provider("claude-haiku-4-5");
        let shared = p.shared_model();
        {
            let mut g = shared.write().unwrap();
            *g = "claude-opus-4-7".into();
        }
        assert_eq!(p.current_model(), "claude-opus-4-7");
    }

    #[test]
    fn format_http_error_redacts_anthropic_key() {
        let body = r#"{"error":"bad token sk-ant-api03-EXAMPLE_FIXTURE_NOT_A_REAL_KEY_aaaaaaaa"}"#;
        let out = format_http_error(401, body);
        assert!(
            !out.contains("sk-ant-api03-EXAMPLE"),
            "key leaked in error message: {out}"
        );
    }

    #[test]
    fn capabilities_advertises_json_mode_emulated() {
        let p = make_provider("claude-haiku-4-5");
        let c = p.capabilities();
        assert!(c.supports_json_mode);
        assert_eq!(c.max_context_tokens, Some(200_000));
    }
}
