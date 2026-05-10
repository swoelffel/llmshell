//! `SwappableProvider` — a `LlmProvider` whose underlying implementation
//! can be replaced at runtime (used to support `/provider set <name>`
//! switching between e.g. OpenAI and Ollama mid-session).
//!
//! All other components (agent loop, compactor, REPL, status prompt) keep
//! holding the same `Arc<dyn LlmProvider>` and `Arc<RwLock<String>>` model
//! label across swaps; only the inner provider behind this wrapper changes.

use async_trait::async_trait;
use llmsh_llm::capabilities::Capabilities;
use llmsh_llm::provider::LlmProvider;
use llmsh_llm::types::{LlmRequest, LlmResponse, ModelInfo};
use std::sync::{Arc, RwLock};

pub struct SwappableProvider {
    inner: RwLock<Arc<dyn LlmProvider>>,
    /// Canonical, externally-shared model label. Updated on `set_model` and
    /// on `swap`. The status prompt and audit emitter read from this Arc.
    shared_model: Arc<RwLock<String>>,
}

impl SwappableProvider {
    pub fn new(initial: Arc<dyn LlmProvider>, shared_model: Arc<RwLock<String>>) -> Self {
        Self {
            inner: RwLock::new(initial),
            shared_model,
        }
    }

    pub fn shared_model(&self) -> Arc<RwLock<String>> {
        self.shared_model.clone()
    }

    /// Replace the inner provider. The caller must have already configured
    /// `new_provider` with the desired model id; this method also mirrors that
    /// id into the shared label.
    pub fn swap(&self, new_provider: Arc<dyn LlmProvider>, new_model: &str) {
        if let Ok(mut g) = self.inner.write() {
            *g = new_provider;
        }
        if let Ok(mut g) = self.shared_model.write() {
            *g = new_model.to_string();
        }
    }

    fn current_inner(&self) -> Arc<dyn LlmProvider> {
        self.inner
            .read()
            .map(|g| g.clone())
            .unwrap_or_else(|p| p.into_inner().clone())
    }
}

#[async_trait]
impl LlmProvider for SwappableProvider {
    fn capabilities(&self) -> Capabilities {
        self.current_inner().capabilities()
    }

    async fn complete(&self, req: LlmRequest) -> anyhow::Result<LlmResponse> {
        let inner = self.current_inner();
        inner.complete(req).await
    }

    async fn list_models(&self) -> anyhow::Result<Vec<ModelInfo>> {
        let inner = self.current_inner();
        inner.list_models().await
    }

    async fn set_model(&self, id: &str) -> anyhow::Result<()> {
        let inner = self.current_inner();
        inner.set_model(id).await?;
        if let Ok(mut g) = self.shared_model.write() {
            *g = id.to_string();
        }
        Ok(())
    }

    fn current_model(&self) -> String {
        self.shared_model
            .read()
            .map(|g| g.clone())
            .unwrap_or_else(|_| self.current_inner().current_model())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use llmsh_llm::capabilities::ToolCallingMode;
    use llmsh_llm::types::{FinishReason, LlmRequest, LlmResponse, MessageRole, ToolPolicyHint};

    struct StubProvider {
        name: &'static str,
        model: RwLock<String>,
    }

    #[async_trait]
    impl LlmProvider for StubProvider {
        fn capabilities(&self) -> Capabilities {
            Capabilities {
                tool_calling: ToolCallingMode::None,
                supports_streaming: false,
                supports_json_mode: false,
                supports_parallel_tool_calls: false,
                supports_tool_choice_required: false,
                max_context_tokens: None,
            }
        }
        async fn complete(&self, _req: LlmRequest) -> anyhow::Result<LlmResponse> {
            Ok(LlmResponse {
                message: Some(format!("{}:{}", self.name, self.current_model())),
                tool_calls: vec![],
                finish_reason: FinishReason::Stop,
                usage: None,
            })
        }
        async fn set_model(&self, id: &str) -> anyhow::Result<()> {
            *self.model.write().unwrap() = id.into();
            Ok(())
        }
        fn current_model(&self) -> String {
            self.model.read().unwrap().clone()
        }
    }

    fn req() -> LlmRequest {
        LlmRequest {
            system: None,
            messages: vec![llmsh_llm::types::Message {
                role: MessageRole::User,
                content: "ping".into(),
                tool_call_id: None,
                name: None,
                tool_calls: None,
            }],
            tools: vec![],
            tool_policy: ToolPolicyHint::None,
            response_format: None,
        }
    }

    #[tokio::test]
    async fn swap_redirects_complete_calls() {
        let a: Arc<dyn LlmProvider> = Arc::new(StubProvider {
            name: "openai",
            model: RwLock::new("gpt-4.1-mini".into()),
        });
        let shared = Arc::new(RwLock::new("gpt-4.1-mini".into()));
        let sp = SwappableProvider::new(a, shared.clone());

        let r1 = sp.complete(req()).await.unwrap();
        assert_eq!(r1.message.as_deref(), Some("openai:gpt-4.1-mini"));

        let b: Arc<dyn LlmProvider> = Arc::new(StubProvider {
            name: "ollama",
            model: RwLock::new("llama3.1:8b".into()),
        });
        sp.swap(b, "llama3.1:8b");

        let r2 = sp.complete(req()).await.unwrap();
        assert_eq!(r2.message.as_deref(), Some("ollama:llama3.1:8b"));
        assert_eq!(*shared.read().unwrap(), "llama3.1:8b");
    }

    #[tokio::test]
    async fn set_model_updates_shared_label() {
        let a: Arc<dyn LlmProvider> = Arc::new(StubProvider {
            name: "openai",
            model: RwLock::new("gpt-4.1-mini".into()),
        });
        let shared = Arc::new(RwLock::new("gpt-4.1-mini".into()));
        let sp = SwappableProvider::new(a, shared.clone());

        sp.set_model("gpt-5").await.unwrap();
        assert_eq!(*shared.read().unwrap(), "gpt-5");
        assert_eq!(sp.current_model(), "gpt-5");
    }
}
