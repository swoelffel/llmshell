mod common;

use async_trait::async_trait;
use llmsh_audit::redact::Redactor;
use llmsh_audit::writer::AuditWriter;
use llmsh_core::agent::{AgentBounds, AgentDeps, AgentLoop};
use llmsh_core::config::{CompactConfig, MemoryConfig};
use llmsh_core::confirm::AlwaysYesGate;
use llmsh_core::context::{ContextBuilder, MemorySystemPrompt};
use llmsh_core::executor::ToolExecutor;
use llmsh_core::memory::Memory;
use llmsh_core::model_cmd::{set_model_flow, ModelCommandContext, ModelListCache};
use llmsh_core::pipeline::Pipeline;
use llmsh_llm::capabilities::{Capabilities, ToolCallingMode};
use llmsh_llm::provider::LlmProvider;
use llmsh_llm::types::{FinishReason, LlmRequest, LlmResponse, ModelInfo};
use llmsh_policy::context::PolicyContext;
use llmsh_policy::engine::{DefaultPolicyConfig, DefaultPolicyEngine};
use llmsh_tools::registry::ToolRegistry;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

struct MockModelProvider {
    model: Arc<RwLock<String>>,
    scripted: Mutex<Vec<LlmResponse>>,
    captured: Mutex<Vec<LlmRequest>>,
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

    async fn complete(&self, req: LlmRequest) -> anyhow::Result<LlmResponse> {
        self.captured.lock().unwrap().push(req);
        let mut s = self.scripted.lock().unwrap();
        if s.is_empty() {
            anyhow::bail!("no scripted responses");
        }
        Ok(s.remove(0))
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
        scripted: Mutex::new(vec![]),
        captured: Mutex::new(vec![]),
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
        model_provider_prefix: None,
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
        scripted: Mutex::new(vec![]),
        captured: Mutex::new(vec![]),
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
        model_provider_prefix: None,
    };

    let before = model_label.read().unwrap().clone();
    set_model_flow(&ctx, "completely-unknown-xyz")
        .await
        .unwrap();
    let after = model_label.read().unwrap().clone();

    assert_eq!(
        before, after,
        "model_label should not change for unknown model"
    );
}

fn stop_response(text: &str) -> LlmResponse {
    LlmResponse {
        message: Some(text.to_string()),
        tool_calls: vec![],
        finish_reason: FinishReason::Stop,
        usage: None,
    }
}

#[tokio::test]
async fn rendered_system_prompt_reflects_model_switch() {
    let workspace = tempfile::tempdir().unwrap();
    let audit_dir = tempfile::tempdir().unwrap();
    let canonical_ws =
        std::fs::canonicalize(workspace.path()).unwrap_or_else(|_| workspace.path().to_path_buf());

    let model_label = Arc::new(RwLock::new("gpt-4o-mini".to_string()));
    let provider = Arc::new(MockModelProvider {
        model: model_label.clone(),
        scripted: Mutex::new(vec![stop_response("turn1"), stop_response("turn2")]),
        captured: Mutex::new(vec![]),
    });

    let memory = Arc::new(Memory::open_in_memory().unwrap());
    let system_prompt = Arc::new(MemorySystemPrompt::new(
        None,
        memory.clone(),
        canonical_ws.clone(),
        model_label.clone(),
        Instant::now(),
    ));

    let registry = Arc::new(ToolRegistry::new());
    let policy = Arc::new(DefaultPolicyEngine::new(DefaultPolicyConfig::default()));
    let pipeline = Pipeline {
        registry: registry.clone(),
        policy,
        home: None,
    };
    let writer = AuditWriter::open(audit_dir.path(), "test-prompt-update").unwrap();

    let deps = Arc::new(AgentDeps {
        provider: provider.clone(),
        pipeline,
        executor: ToolExecutor {
            registry,
            timeout: Duration::from_secs(5),
            max_output_bytes: 4096,
            env: Default::default(),
            cancel: CancellationToken::new(),
        },
        gate: Arc::new(AlwaysYesGate),
        audit: Mutex::new(writer),
        redactor: Redactor::default_audit(),
        bounds: AgentBounds {
            max_iterations: 5,
            max_tool_calls_per_iteration: 5,
            max_schema_repair_attempts: 2,
        },
        compact_config: CompactConfig::default(),
        memory_cfg: MemoryConfig::default(),
        policy_ctx: PolicyContext {
            cwd: canonical_ws.clone(),
            workspace_root: canonical_ws.clone(),
            allowed_roots: vec![canonical_ws.clone()],
            sensitive_path_patterns: vec![],
        },
        sensitive_patterns: vec![],
        model_label: model_label.clone(),
        system_prompt,
        memory,
        verbose: 0,
        stats: Arc::new(std::sync::RwLock::new(
            llmsh_core::session_stats::SessionStats::default(),
        )),
    });

    // Turn 1: capture initial system prompt.
    {
        let mut agent = AgentLoop {
            deps: deps.clone(),
            builder: ContextBuilder::new(4096),
        };
        agent.run("hello").await.unwrap();
    }
    let first_system = provider.captured.lock().unwrap()[0]
        .system
        .clone()
        .unwrap_or_default();
    assert!(
        first_system.contains("model: gpt-4o-mini"),
        "initial render must show model: gpt-4o-mini, got:\n{}",
        first_system
    );

    // Switch model via the same flow the REPL uses.
    let cache = ModelListCache::new();
    let ctx = ModelCommandContext {
        provider: provider.as_ref(),
        model_label: &model_label,
        cache: &cache,
        config_path: None,
        audit: &deps.audit,
        model_provider_prefix: None,
    };
    set_model_flow(&ctx, "gpt-4o").await.unwrap();

    // Turn 2: capture system prompt after switch.
    {
        let mut agent = AgentLoop {
            deps: deps.clone(),
            builder: ContextBuilder::new(4096),
        };
        agent.run("again").await.unwrap();
    }
    let second_system = provider.captured.lock().unwrap()[1]
        .system
        .clone()
        .unwrap_or_default();
    assert!(
        second_system.contains("model: gpt-4o"),
        "after switch the prompt must show model: gpt-4o, got:\n{}",
        second_system
    );
    assert!(
        !second_system.contains("model: gpt-4o-mini"),
        "old model id must not appear after switch, got:\n{}",
        second_system
    );
}

#[tokio::test]
async fn set_model_flow_persists_with_provider_prefix() {
    let tmp_audit = tempfile::tempdir().unwrap();
    let cfg_dir = tempfile::tempdir().unwrap();
    let cfg_path = cfg_dir.path().join("config.toml");
    let original = r#"# user config
default_model = "openai:gpt-4o-mini"

[providers.openai]
api_key_env = "OPENAI_API_KEY"
base_url = "https://api.openai.com/v1"
tool_calling = "native"
"#;
    std::fs::write(&cfg_path, original).unwrap();

    // The runtime label holds only the bare id (matches what build_provider produces).
    let model_label = Arc::new(RwLock::new("gpt-4o-mini".to_string()));
    let provider = MockModelProvider {
        model: model_label.clone(),
        scripted: Mutex::new(vec![]),
        captured: Mutex::new(vec![]),
    };
    let writer = AuditWriter::open(tmp_audit.path(), "test-persist-prefix").unwrap();
    let audit = Mutex::new(writer);
    let cache = ModelListCache::new();

    let ctx = ModelCommandContext {
        provider: &provider,
        model_label: &model_label,
        cache: &cache,
        config_path: Some(cfg_path.as_path()),
        audit: &audit,
        model_provider_prefix: Some("openai".into()),
    };

    set_model_flow(&ctx, "gpt-4o").await.unwrap();

    let result = std::fs::read_to_string(&cfg_path).unwrap();
    assert!(
        result.contains("default_model = \"openai:gpt-4o\""),
        "must persist with provider prefix, got:\n{}",
        result
    );
    assert!(
        result.contains("# user config"),
        "comment must be preserved, got:\n{}",
        result
    );
    assert!(
        result.contains("api_key_env = \"OPENAI_API_KEY\""),
        "provider section must be preserved, got:\n{}",
        result
    );
}
