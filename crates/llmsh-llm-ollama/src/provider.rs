use crate::mapping::*;
use crate::wire::*;
use anyhow::anyhow;
use async_trait::async_trait;
use llmsh_llm::capabilities::{Capabilities, ToolCallingMode};
use llmsh_llm::provider::LlmProvider;
use llmsh_llm::types::{LlmRequest, LlmResponse, ModelInfo};
use std::sync::{Arc, RwLock};

pub struct OllamaProvider {
    base_url: String,
    model: Arc<RwLock<String>>,
    http: reqwest::Client,
}

pub struct OllamaConfig {
    pub base_url: String,
    pub model: String,
    pub timeout_ms: u64,
}

impl OllamaProvider {
    pub fn new(cfg: OllamaConfig) -> anyhow::Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(cfg.timeout_ms))
            .build()?;
        Ok(Self {
            base_url: cfg.base_url,
            model: Arc::new(RwLock::new(cfg.model)),
            http,
        })
    }

    pub fn shared_model(&self) -> Arc<RwLock<String>> {
        self.model.clone()
    }
}

#[async_trait]
impl LlmProvider for OllamaProvider {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            tool_calling: ToolCallingMode::Native,
            supports_streaming: false,
            supports_json_mode: true,
            supports_parallel_tool_calls: true,
            // Ollama has no equivalent to OpenAI's tool_choice="required".
            supports_tool_choice_required: false,
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
        let format = req.response_format.as_ref().and_then(|f| match f {
            llmsh_llm::types::ResponseFormat::Text => None,
            llmsh_llm::types::ResponseFormat::JsonObject => Some("json"),
        });
        let body = ChatRequest {
            model: &model,
            messages,
            tools,
            stream: false,
            format,
        };
        let url = format!("{}/api/chat", self.base_url.trim_end_matches('/'));
        let resp = self.http.post(&url).json(&body).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("ollama http {}: {}", status, text);
        }
        let parsed: ChatResponse = resp.json().await?;
        parse_response(parsed)
    }

    async fn list_models(&self) -> anyhow::Result<Vec<ModelInfo>> {
        let url = format!("{}/api/tags", self.base_url.trim_end_matches('/'));
        let resp = self.http.get(&url).send().await?.error_for_status()?;
        let body: TagsResponse = resp.json().await?;
        Ok(body
            .models
            .into_iter()
            .map(|e| ModelInfo {
                id: e.name,
                owned_by: Some("ollama".into()),
                created: e.modified_at.and_then(parse_iso_to_epoch),
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

fn parse_iso_to_epoch(_s: String) -> Option<i64> {
    // Ollama's modified_at is RFC3339 with sub-second precision; we don't
    // currently surface it, so just drop to None rather than pulling chrono in.
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_provider(model: &str) -> OllamaProvider {
        OllamaProvider::new(OllamaConfig {
            base_url: "http://localhost:11434".into(),
            model: model.into(),
            timeout_ms: 5000,
        })
        .unwrap()
    }

    #[test]
    fn current_model_returns_initial() {
        let p = make_provider("llama3.1:8b");
        assert_eq!(p.current_model(), "llama3.1:8b");
    }

    #[tokio::test]
    async fn set_model_updates_current() {
        let p = make_provider("llama3.1:8b");
        p.set_model("qwen2.5-coder:7b").await.unwrap();
        assert_eq!(p.current_model(), "qwen2.5-coder:7b");
    }

    #[test]
    fn shared_model_reflects_mutations() {
        let p = make_provider("llama3.1:8b");
        let shared = p.shared_model();
        {
            let mut g = shared.write().unwrap();
            *g = "mistral-nemo:12b".into();
        }
        assert_eq!(p.current_model(), "mistral-nemo:12b");
    }
}
