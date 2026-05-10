//! Provider hot-swap end-to-end coverage.
//!
//! Drives `/provider set <name>` programmatically, asserting that:
//!   * the SwappableProvider shifts to the new inner provider,
//!   * `default_model` in `config.toml` is rewritten to `<name>:<model>`,
//!   * an `AuditEvent::ProviderChanged` lands in the JSONL chain.
//!
//! Mirrors the pattern in `e2e_model_switch.rs` (stub provider + writer +
//! ModelListCache, no real network IO). The chained `/model` interactive
//! step uses a `Cursor` reader so stdin is never touched.

use async_trait::async_trait;
use llmsh_audit::writer::AuditWriter;
use llmsh_core::config::Config;
use llmsh_core::model_cmd::ModelListCache;
use llmsh_core::provider_cmd::{
    set_provider_flow_with_reader, ProviderCommandContext, ProviderSwapper,
};
use llmsh_core::swappable::SwappableProvider;
use llmsh_llm::capabilities::{Capabilities, ToolCallingMode};
use llmsh_llm::provider::LlmProvider;
use llmsh_llm::types::{FinishReason, LlmRequest, LlmResponse, ModelInfo};
use std::io::Cursor;
use std::sync::{Arc, Mutex, RwLock};

struct StubProvider {
    name: String,
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
    async fn list_models(&self) -> anyhow::Result<Vec<ModelInfo>> {
        Ok(vec![ModelInfo {
            id: self.current_model(),
            owned_by: Some(self.name.clone()),
            created: None,
        }])
    }
    async fn set_model(&self, id: &str) -> anyhow::Result<()> {
        *self.model.write().unwrap() = id.into();
        Ok(())
    }
    fn current_model(&self) -> String {
        self.model.read().unwrap().clone()
    }
}

struct StubSwapper;

impl ProviderSwapper for StubSwapper {
    fn build(
        &self,
        name: &str,
        model: &str,
        _cfg: &Config,
    ) -> anyhow::Result<Arc<dyn LlmProvider>> {
        Ok(Arc::new(StubProvider {
            name: name.into(),
            model: RwLock::new(model.into()),
        }))
    }
}

fn cfg_with_two_providers() -> Config {
    let mut cfg = Config::defaults();
    // Trim allowlists down to one model each so the chained /model step is
    // deterministic (a single-entry list with the active model selected
    // implicitly when the user presses Enter).
    cfg.providers.get_mut("openai").unwrap().models = vec!["gpt-4.1-mini".into()];
    cfg.providers.get_mut("ollama").unwrap().models = vec!["llama3.1:8b".into()];
    cfg.default_model = "openai:gpt-4.1-mini".into();
    cfg
}

#[tokio::test]
async fn provider_switch_swaps_inner_persists_and_audits() {
    let tmp_audit = tempfile::tempdir().unwrap();
    let tmp_cfg = tempfile::tempdir().unwrap();
    let cfg_path = tmp_cfg.path().join("config.toml");
    std::fs::write(
        &cfg_path,
        "default_model = \"openai:gpt-4.1-mini\"\n\
         [providers.openai]\n\
         api_key_env = \"OPENAI_API_KEY\"\n\
         base_url = \"https://api.openai.com/v1\"\n\
         tool_calling = \"native\"\n\
         [providers.ollama]\n\
         base_url = \"http://localhost:11434\"\n\
         tool_calling = \"native\"\n",
    )
    .unwrap();

    let cfg = cfg_with_two_providers();
    let initial: Arc<dyn LlmProvider> = Arc::new(StubProvider {
        name: "openai".into(),
        model: RwLock::new("gpt-4.1-mini".into()),
    });
    let shared = Arc::new(RwLock::new("gpt-4.1-mini".into()));
    let swappable = Arc::new(SwappableProvider::new(initial, shared.clone()));

    let writer = AuditWriter::open(tmp_audit.path(), "test-provider-switch").unwrap();
    let audit = Mutex::new(writer);
    let cache = ModelListCache::new();
    let swapper = StubSwapper;
    let current = Some("openai".into());

    let mut ctx = ProviderCommandContext {
        cfg: &cfg,
        current_provider: &current,
        swappable: &swappable,
        model_cache: &cache,
        audit: &audit,
        config_path: Some(cfg_path.as_path()),
        swapper: &swapper,
    };

    // Empty input → chained interactive_select reads 0 bytes, keeps the
    // freshly-set model, and returns Ok.
    let mut reader: Cursor<&[u8]> = Cursor::new(b"");
    let result = set_provider_flow_with_reader(&mut ctx, "ollama", &mut reader)
        .await
        .unwrap();
    assert_eq!(result.as_deref(), Some("ollama"));

    // Shared label tracks the new provider's first allowlisted model.
    assert_eq!(*shared.read().unwrap(), "llama3.1:8b");

    // Swappable now answers as the new inner provider.
    let r = swappable
        .complete(LlmRequest {
            system: None,
            messages: vec![],
            tools: vec![],
            tool_policy: llmsh_llm::types::ToolPolicyHint::None,
            response_format: None,
        })
        .await
        .unwrap();
    assert_eq!(r.message.as_deref(), Some("ollama:llama3.1:8b"));

    // Config persisted: default_model rewritten with the new provider:model.
    let cfg_after = std::fs::read_to_string(&cfg_path).unwrap();
    assert!(
        cfg_after.contains("default_model = \"ollama:llama3.1:8b\""),
        "default_model not rewritten: {}",
        cfg_after
    );

    // Audit chain contains a ProviderChanged event with both ends.
    audit.lock().unwrap().flush().unwrap();
    let log = std::fs::read_to_string(tmp_audit.path().join("test-provider-switch.jsonl")).unwrap();
    assert!(
        log.contains("\"type\":\"provider_changed\""),
        "provider_changed event missing: {}",
        log
    );
    assert!(log.contains("\"from\":\"openai\""), "from field missing");
    assert!(log.contains("\"to\":\"ollama\""), "to field missing");
}

#[tokio::test]
async fn provider_switch_to_same_provider_is_noop() {
    let tmp_audit = tempfile::tempdir().unwrap();
    let cfg = cfg_with_two_providers();
    let initial: Arc<dyn LlmProvider> = Arc::new(StubProvider {
        name: "openai".into(),
        model: RwLock::new("gpt-4.1-mini".into()),
    });
    let shared = Arc::new(RwLock::new("gpt-4.1-mini".into()));
    let swappable = Arc::new(SwappableProvider::new(initial, shared.clone()));

    let writer = AuditWriter::open(tmp_audit.path(), "test-provider-noop").unwrap();
    let audit = Mutex::new(writer);
    let cache = ModelListCache::new();
    let swapper = StubSwapper;
    let current = Some("openai".into());

    let mut ctx = ProviderCommandContext {
        cfg: &cfg,
        current_provider: &current,
        swappable: &swappable,
        model_cache: &cache,
        audit: &audit,
        config_path: None,
        swapper: &swapper,
    };

    let mut reader: Cursor<&[u8]> = Cursor::new(b"");
    let result = set_provider_flow_with_reader(&mut ctx, "openai", &mut reader)
        .await
        .unwrap();
    assert_eq!(result, None);
    assert_eq!(*shared.read().unwrap(), "gpt-4.1-mini");

    audit.lock().unwrap().flush().unwrap();
    let log = std::fs::read_to_string(tmp_audit.path().join("test-provider-noop.jsonl")).unwrap();
    assert!(
        !log.contains("\"type\":\"provider_changed\""),
        "no provider_changed event expected on noop: {}",
        log
    );
}
