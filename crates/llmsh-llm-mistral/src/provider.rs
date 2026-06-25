use crate::mapping::*;
use crate::wire::*;
use anyhow::anyhow;
use async_trait::async_trait;
use llmsh_llm::capabilities::{Capabilities, ToolCallingMode};
use llmsh_llm::provider::LlmProvider;
use llmsh_llm::types::{LlmRequest, LlmResponse, ModelInfo};
use secrecy::{ExposeSecret, SecretString};
use std::sync::{Arc, RwLock};

pub struct MistralProvider {
    base_url: String,
    api_key: SecretString,
    model: Arc<RwLock<String>>,
    http: reqwest::Client,
}

impl std::fmt::Debug for MistralProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MistralProvider")
            .field("base_url", &self.base_url)
            .field("api_key", &"[REDACTED]")
            .field("model", &self.model)
            .finish()
    }
}

pub struct MistralConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub timeout_ms: u64,
}

impl MistralProvider {
    pub fn new(cfg: MistralConfig) -> anyhow::Result<Self> {
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
impl LlmProvider for MistralProvider {
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
        let tool_choice = if tools.is_empty() {
            None
        } else {
            tool_choice_for(req.tool_policy)
        };
        let parallel_tool_calls = if tools.is_empty() { None } else { Some(true) };
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
            parallel_tool_calls,
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
            anyhow::bail!(
                "{}",
                format_http_error(status.as_u16(), &text, self.api_key.expose_secret())
            );
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
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!(
                "{}",
                format_http_error(status.as_u16(), &text, self.api_key.expose_secret())
            );
        }
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

fn format_http_error(status: u16, body: &str, api_key: &str) -> String {
    let redacted = llmsh_redact::Redactor::default().redact(body);
    let redacted = if api_key.is_empty() {
        redacted
    } else {
        redacted.replace(api_key, "[REDACTED:mistral_key]")
    };
    format!("mistral http {status}: {redacted}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_provider(model: &str) -> MistralProvider {
        MistralProvider::new(MistralConfig {
            base_url: "https://api.mistral.ai/v1".into(),
            api_key: "mistral-api-key-EXAMPLE_FIXTURE_NOT_REAL".into(),
            model: model.into(),
            timeout_ms: 5000,
        })
        .unwrap()
    }

    #[test]
    fn current_model_returns_initial() {
        let p = make_provider("mistral-small-latest");
        assert_eq!(p.current_model(), "mistral-small-latest");
    }

    #[tokio::test]
    async fn set_model_updates_current() {
        let p = make_provider("mistral-small-latest");
        p.set_model("mistral-medium-latest").await.unwrap();
        assert_eq!(p.current_model(), "mistral-medium-latest");
    }

    #[test]
    fn debug_does_not_leak_api_key() {
        let p = make_provider("mistral-small-latest");
        let dbg = format!("{:?}", p);
        assert!(!dbg.contains("mistral-api-key-EXAMPLE_FIXTURE_NOT_REAL"));
    }

    #[test]
    fn shared_model_reflects_mutations() {
        let p = make_provider("mistral-small-latest");
        let shared = p.shared_model();
        {
            let mut g = shared.write().unwrap();
            *g = "mistral-medium-latest".into();
        }
        assert_eq!(p.current_model(), "mistral-medium-latest");
    }

    #[test]
    fn http_error_redacts_exact_configured_key() {
        let out = format_http_error(
            401,
            r#"{"error":"bad token mistral-api-key-EXAMPLE_FIXTURE_NOT_REAL"}"#,
            "mistral-api-key-EXAMPLE_FIXTURE_NOT_REAL",
        );
        assert!(out.contains("[REDACTED:mistral_key]"));
        assert!(!out.contains("mistral-api-key-EXAMPLE"));
    }
}
