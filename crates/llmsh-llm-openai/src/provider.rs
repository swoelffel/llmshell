use crate::mapping::*;
use crate::wire::*;
use async_trait::async_trait;
use llmsh_llm::capabilities::{Capabilities, ToolCallingMode};
use llmsh_llm::provider::LlmProvider;
use llmsh_llm::types::{LlmRequest, LlmResponse};

pub struct OpenAIProvider {
    base_url: String,
    api_key: String,
    model: String,
    http: reqwest::Client,
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
            api_key: cfg.api_key,
            model: cfg.model,
            http,
        })
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
        let messages = to_wire_messages(req.system.as_deref(), &req.messages);
        let tools = to_wire_tools(&req.tools);
        let tool_choice = tool_choice_for(req.tool_policy);
        let body = ChatRequest {
            model: &self.model,
            messages,
            tools,
            tool_choice,
        };
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            anyhow::bail!("openai http {}: {}", status, text);
        }
        let parsed: ChatResponse = resp.json().await?;
        parse_response(parsed)
    }
}
