//! Verifies that SessionStats are correctly accumulated across two AgentLoop turns.

mod common;

use async_trait::async_trait;
use llmsh_audit::redact::Redactor;
use llmsh_audit::writer::AuditWriter;
use llmsh_core::agent::{AgentBounds, AgentDeps, AgentLoop};
use llmsh_core::config::{CompactConfig, MemoryConfig};
use llmsh_core::confirm::AlwaysYesGate;
use llmsh_core::context::{ContextBuilder, StaticSystemPrompt};
use llmsh_core::executor::ToolExecutor;
use llmsh_core::memory::Memory;
use llmsh_core::pipeline::Pipeline;
use llmsh_core::session_stats::SessionStats;
use llmsh_llm::capabilities::{Capabilities, ToolCallingMode};
use llmsh_llm::provider::LlmProvider;
use llmsh_llm::types::{FinishReason, LlmRequest, LlmResponse, ModelInfo, TokenUsage};
use llmsh_policy::context::PolicyContext;
use llmsh_policy::engine::{DefaultPolicyConfig, DefaultPolicyEngine};
use llmsh_tools::registry::ToolRegistry;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

struct ScriptedProvider {
    responses: Mutex<Vec<LlmResponse>>,
    model: Arc<RwLock<String>>,
}

#[async_trait]
impl LlmProvider for ScriptedProvider {
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
    async fn complete(&self, _: LlmRequest) -> anyhow::Result<LlmResponse> {
        Ok(self.responses.lock().unwrap().remove(0))
    }
    async fn list_models(&self) -> anyhow::Result<Vec<ModelInfo>> {
        Ok(vec![])
    }
    async fn set_model(&self, _: &str) -> anyhow::Result<()> {
        Ok(())
    }
    fn current_model(&self) -> String {
        self.model.read().unwrap().clone()
    }
}

/// Two separate user turns each return a single Stop response.  The shared
/// `SessionStats` handle must accumulate both turns' token counts correctly.
#[tokio::test]
async fn session_stats_accumulate_across_turns() {
    let audit_dir = tempfile::tempdir().unwrap();

    let model_label: Arc<RwLock<String>> = Arc::new(RwLock::new("openai:gpt-4o-mini".into()));

    let responses = vec![
        LlmResponse {
            message: Some("first".into()),
            tool_calls: vec![],
            finish_reason: FinishReason::Stop,
            usage: Some(TokenUsage {
                input_tokens: Some(1000),
                output_tokens: Some(50),
                total_tokens: Some(1050),
                cached_input_tokens: Some(800),
            }),
        },
        LlmResponse {
            message: Some("second".into()),
            tool_calls: vec![],
            finish_reason: FinishReason::Stop,
            usage: Some(TokenUsage {
                input_tokens: Some(500),
                output_tokens: Some(20),
                total_tokens: Some(520),
                cached_input_tokens: Some(0),
            }),
        },
    ];

    let provider = Arc::new(ScriptedProvider {
        responses: Mutex::new(responses),
        model: model_label.clone(),
    });

    let registry = Arc::new(ToolRegistry::new());
    let policy = Arc::new(DefaultPolicyEngine::new(DefaultPolicyConfig::default()));
    let pipeline = Pipeline {
        registry: registry.clone(),
        policy,
        home: None,
    };
    let writer = AuditWriter::open(audit_dir.path(), "test-session").unwrap();

    let stats: Arc<RwLock<SessionStats>> = Arc::new(RwLock::new(SessionStats::default()));

    let tmp = tempfile::tempdir().unwrap();
    let canonical_tmp =
        std::fs::canonicalize(tmp.path()).unwrap_or_else(|_| tmp.path().to_path_buf());

    let deps = Arc::new(AgentDeps {
        provider,
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
            cwd: std::sync::Arc::new(std::sync::RwLock::new(canonical_tmp.clone())),
            workspace_root: canonical_tmp.clone(),
            allowed_roots: vec![canonical_tmp],
            sensitive_path_patterns: vec![],
        },
        sensitive_patterns: vec![],
        model_label,
        system_prompt: Arc::new(StaticSystemPrompt::new(None)),
        memory: Arc::new(Memory::open_in_memory().unwrap()),
        verbose: 0,
        stats: stats.clone(),
        oldpwd: std::sync::Arc::new(std::sync::Mutex::new(None)),
        home: None,
    });

    let mut agent = AgentLoop {
        deps: deps.clone(),
        builder: ContextBuilder::new(4096),
    };

    agent.run("hello").await.unwrap();
    agent.run("again").await.unwrap();

    let s = stats.read().unwrap();
    assert_eq!(s.totals.turns, 2, "expected 2 turns");
    assert_eq!(s.totals.input_tokens, 1500, "expected 1500 input tokens");
    assert_eq!(s.totals.output_tokens, 70, "expected 70 output tokens");
    assert_eq!(
        s.totals.cached_input_tokens, 800,
        "expected 800 cached input tokens"
    );
    assert!(!s.totals.cost_partial, "cost should not be partial");
    assert!(s.totals.cost_usd > 0.0, "cost_usd should be > 0");
}
