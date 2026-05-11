use crate::mapping::*;
use crate::wire::*;
use anyhow::anyhow;
use async_trait::async_trait;
use llmsh_llm::capabilities::{Capabilities, ToolCallingMode};
use llmsh_llm::provider::LlmProvider;
use llmsh_llm::types::{LlmRequest, LlmResponse, ModelInfo};
use secrecy::{ExposeSecret, SecretString};
use std::sync::{Arc, RwLock};

pub struct OpenAIProvider {
    base_url: String,
    api_key: SecretString,
    model: Arc<RwLock<String>>,
    http: reqwest::Client,
}

impl std::fmt::Debug for OpenAIProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAIProvider")
            .field("base_url", &self.base_url)
            .field("api_key", &"[REDACTED]")
            .field("model", &self.model)
            .finish()
    }
}

pub struct OpenAIConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub timeout_ms: u64,
}

impl OpenAIProvider {
    pub fn new(cfg: OpenAIConfig) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(cfg.timeout_ms))
            .build()?;
        Ok(Self {
            base_url: cfg.base_url,
            api_key: SecretString::new(cfg.api_key),
            model: Arc::new(RwLock::new(cfg.model)),
            http,
        })
    }

    pub fn shared_model(&self) -> Arc<RwLock<String>> {
        self.model.clone()
    }
}

#[async_trait]
impl LlmProvider for OpenAIProvider {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            tool_calling: ToolCallingMode::Native,
            supports_streaming: false,
            supports_json_mode: true,
            supports_parallel_tool_calls: true,
            supports_tool_choice_required: true,
            max_context_tokens: None,
        }
    }

    async fn complete(&self, req: LlmRequest) -> anyhow::Result<LlmResponse> {
        let model = self
            .model
            .read()
            .map_err(|_| anyhow!("model lock poisoned"))?
            .clone();
        let messages = to_wire_messages(req.system.as_deref(), &req.messages);
        let tools = to_wire_tools(&req.tools);
        // OpenAI rejects `tool_choice` when `tools` is absent/empty
        // (400 invalid_request_error). Strip it in that case.
        let tool_choice = if tools.is_empty() {
            None
        } else {
            tool_choice_for(req.tool_policy)
        };
        let response_format = req.response_format.as_ref().map(|f| match f {
            llmsh_llm::types::ResponseFormat::Text => WireResponseFormat { kind: "text" },
            llmsh_llm::types::ResponseFormat::JsonObject => WireResponseFormat {
                kind: "json_object",
            },
        });
        let body = ChatRequest {
            model: &model,
            messages,
            tools,
            tool_choice,
            response_format,
        };
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let resp = self
            .http
            .post(&url)
            .bearer_auth(self.api_key.expose_secret())
            .json(&body)
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("{}", format_http_error(status.as_u16(), &text));
        }
        let parsed: ChatResponse = resp.json().await?;
        parse_response(parsed)
    }

    async fn list_models(&self) -> anyhow::Result<Vec<ModelInfo>> {
        let url = format!("{}/models", self.base_url.trim_end_matches('/'));
        let resp = self
            .http
            .get(&url)
            .bearer_auth(self.api_key.expose_secret())
            .send()
            .await?
            .error_for_status()?;
        let body: ModelsResponse = resp.json().await?;
        Ok(body
            .data
            .into_iter()
            .map(|e| ModelInfo {
                id: e.id,
                owned_by: e.owned_by,
                created: e.created,
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

fn format_http_error(status: u16, body: &str) -> String {
    let redacted = llmsh_redact::Redactor::default().redact(body);
    format!("openai http {status}: {redacted}")
}

#[cfg(test)]
mod error_format_tests {
    use super::*;

    #[test]
    fn redacts_secret_in_error_body() {
        let body = r#"{"error":"bad token sk-proj-abcDEF1234567890abcDEF1234567890abcDEF12"}"#;
        let out = format_http_error(401, body);
        assert!(out.contains("[REDACTED:openai_key]"));
        assert!(!out.contains("sk-proj-abcDEF"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_provider(model: &str) -> OpenAIProvider {
        OpenAIProvider::new(OpenAIConfig {
            base_url: "https://api.openai.com/v1".into(),
            api_key: "test-key".into(),
            model: model.into(),
            timeout_ms: 5000,
        })
        .unwrap()
    }

    #[test]
    fn current_model_returns_initial() {
        let p = make_provider("gpt-4o-mini");
        assert_eq!(p.current_model(), "gpt-4o-mini");
    }

    #[tokio::test]
    async fn set_model_updates_current() {
        let p = make_provider("gpt-4o-mini");
        p.set_model("gpt-4o").await.unwrap();
        assert_eq!(p.current_model(), "gpt-4o");
    }

    #[test]
    fn debug_does_not_leak_api_key() {
        let p = OpenAIProvider::new(OpenAIConfig {
            base_url: "https://api.openai.com/v1".into(),
            api_key: "sk-proj-SECRET12345".into(),
            model: "gpt-4".into(),
            timeout_ms: 5000,
        })
        .unwrap();
        let dbg = format!("{:?}", p);
        assert!(
            !dbg.contains("sk-proj-SECRET12345"),
            "debug leaks api key: {dbg}"
        );
    }

    #[test]
    fn shared_model_reflects_mutations() {
        let p = make_provider("gpt-4o-mini");
        let shared = p.shared_model();
        {
            let mut g = shared.write().unwrap();
            *g = "gpt-4o".into();
        }
        assert_eq!(p.current_model(), "gpt-4o");
    }
}
