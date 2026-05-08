mod common;

use async_trait::async_trait;
use llmsh_audit::writer::AuditWriter;
use llmsh_core::model_cmd::{set_model_flow, ModelCommandContext, ModelListCache};
use llmsh_llm::capabilities::{Capabilities, ToolCallingMode};
use llmsh_llm::provider::LlmProvider;
use llmsh_llm::types::{LlmRequest, LlmResponse, ModelInfo};
use std::sync::{Arc, Mutex, RwLock};

struct MockModelProvider {
    model: Arc<RwLock<String>>,
}

#[async_trait]
impl LlmProvider for MockModelProvider {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            tool_calling: ToolCallingMode::Native,
            supports_streaming: false,
            supports_json_mode: true,
            supports_parallel_tool_calls: false,
            supports_tool_choice_required: true,
            max_context_tokens: None,
        }
    }

    async fn complete(&self, _req: LlmRequest) -> anyhow::Result<LlmResponse> {
        anyhow::bail!("not implemented in mock")
    }

    async fn list_models(&self) -> anyhow::Result<Vec<ModelInfo>> {
        Ok(vec![
            ModelInfo {
                id: "gpt-4o-mini".into(),
                owned_by: None,
                created: None,
            },
            ModelInfo {
                id: "gpt-4o".into(),
                owned_by: None,
                created: None,
            },
            ModelInfo {
                id: "whisper-1".into(),
                owned_by: None,
                created: None,
            },
        ])
    }

    async fn set_model(&self, id: &str) -> anyhow::Result<()> {
        let mut g = self.model.write().unwrap();
        *g = id.to_string();
        Ok(())
    }

    fn current_model(&self) -> String {
        self.model
            .read()
            .map(|g| g.clone())
            .unwrap_or_else(|_| "unknown".into())
    }
}

#[tokio::test]
async fn set_model_flow_updates_shared_label() {
    let tmp_audit = tempfile::tempdir().unwrap();
    let model_label = Arc::new(RwLock::new("openai:gpt-4o-mini".to_string()));
    let provider = MockModelProvider {
        model: model_label.clone(),
    };
    let writer = AuditWriter::open(tmp_audit.path(), "test-model-switch").unwrap();
    let audit = Mutex::new(writer);
    let cache = ModelListCache::new();

    let ctx = ModelCommandContext {
        provider: &provider,
        model_label: &model_label,
        cache: &cache,
        config_path: None,
        audit: &audit,
    };

    set_model_flow(&ctx, "gpt-4o").await.unwrap();

    let current = model_label.read().unwrap().clone();
    assert_eq!(current, "gpt-4o", "model_label should be updated after set");
}

#[tokio::test]
async fn set_model_flow_unknown_model_prints_error() {
    let tmp_audit = tempfile::tempdir().unwrap();
    let model_label = Arc::new(RwLock::new("openai:gpt-4o-mini".to_string()));
    let provider = MockModelProvider {
        model: model_label.clone(),
    };
    let writer = AuditWriter::open(tmp_audit.path(), "test-model-switch-err").unwrap();
    let audit = Mutex::new(writer);
    let cache = ModelListCache::new();

    let ctx = ModelCommandContext {
        provider: &provider,
        model_label: &model_label,
        cache: &cache,
        config_path: None,
        audit: &audit,
    };

    // Should not error but print to stderr and leave model unchanged
    let before = model_label.read().unwrap().clone();
    set_model_flow(&ctx, "completely-unknown-xyz")
        .await
        .unwrap();
    let after = model_label.read().unwrap().clone();

    // Model label is not updated because OpenAI-style set_model was not called
    // The model_label holds the Arc so if provider.set_model was not called it stays
    assert_eq!(
        before, after,
        "model_label should not change for unknown model"
    );
}
