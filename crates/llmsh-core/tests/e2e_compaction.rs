//! End-to-end coverage for the cascade compactor.

use async_trait::async_trait;
use llmsh_audit::redact::Redactor;
use llmsh_audit::writer::AuditWriter;
use llmsh_core::agent::{AgentBounds, AgentDeps, AgentLoop};
use llmsh_core::compactor::validate::validate_no_orphans;
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

fn stop(text: &str, input: u32) -> LlmResponse {
    LlmResponse {
        message: Some(text.into()),
        tool_calls: vec![],
        finish_reason: FinishReason::Stop,
        usage: Some(TokenUsage {
            input_tokens: Some(input),
            output_tokens: Some(20),
            total_tokens: Some(input + 20),
            cached_input_tokens: Some(0),
        }),
    }
}

fn build_deps(
    provider: Arc<dyn LlmProvider>,
    compact_config: CompactConfig,
    model: Arc<RwLock<String>>,
) -> Arc<AgentDeps> {
    let registry = Arc::new(ToolRegistry::new());
    let policy = Arc::new(DefaultPolicyEngine::new(DefaultPolicyConfig {
        risk_actions: Default::default(),
        sensitive_paths_action: llmsh_policy::engine::RiskAction::Confirm,
        allow_outside_workspace: true,
    }));
    let pipeline = Pipeline {
        registry: registry.clone(),
        policy: policy.clone(),
        home: None,
    };
    let executor = ToolExecutor {
        registry: registry.clone(),
        timeout: std::time::Duration::from_millis(5_000),
        max_output_bytes: 4096,
        env: Default::default(),
        cancel: CancellationToken::new(),
    };
    let memory = Arc::new(Memory::open_in_memory().expect("memory"));
    let audit = std::sync::Mutex::new(AuditWriter::disabled());
    Arc::new(AgentDeps {
        provider,
        pipeline,
        executor,
        gate: Arc::new(AlwaysYesGate),
        audit,
        redactor: Redactor::default_audit(),
        bounds: AgentBounds {
            max_iterations: 5,
            max_tool_calls_per_iteration: 5,
            max_schema_repair_attempts: 1,
        },
        policy_ctx: PolicyContext {
            cwd: std::env::temp_dir(),
            workspace_root: std::env::temp_dir(),
            allowed_roots: vec![std::env::temp_dir()],
            sensitive_path_patterns: vec![],
        },
        sensitive_patterns: vec![],
        model_label: model.clone(),
        system_prompt: Arc::new(StaticSystemPrompt::new(None)),
        memory,
        verbose: 0,
        stats: Arc::new(RwLock::new(SessionStats::default())),
        compact_config,
        memory_cfg: MemoryConfig::default(),
    })
}

/// Verify that auto-compaction fires once the prior turn's input_tokens crosses
/// the configured threshold.
///
/// Timeline (model = gpt-4o-mini, window = 128_000, threshold = 1% = 1_280):
///   Turn 1 (q1): no last_turn yet → no compaction. Response 0 = ans1 (input=1_000).
///   Turn 2 (q2): 1_000 < 1_280 → no compaction. Response 1 = ans2 (input=100_000).
///   Turn 3 (q3): 100_000 >= 1_280 → compaction triggered.
///     summarize_prefix consumes Response 2 = "résumé condensé".
///     Then LLM call consumes Response 3 = ans3 (input=50_000).
///   Turn 4 (q4): 50_000 >= 1_280 → compaction triggered again.
///     summarize_prefix consumes Response 4 = "résumé2".
///     Then LLM call consumes Response 5 = ans4 (input=30_000).
#[tokio::test]
async fn auto_compaction_triggers_when_threshold_crossed() {
    let model = Arc::new(RwLock::new("openai:gpt-4o-mini".to_string()));
    let provider: Arc<dyn LlmProvider> = Arc::new(ScriptedProvider {
        responses: Mutex::new(vec![
            stop("ans1", 1_000),
            stop("ans2", 100_000),
            stop("résumé condensé", 50),
            stop("ans3", 50_000),
            stop("résumé2", 50),
            stop("ans4", 30_000),
        ]),
        model: model.clone(),
    });

    let compact_config = CompactConfig {
        auto_threshold_pct: 1,
        keep_last_user_turns: 2,
        tool_output_max_bytes: 2048,
        summary_max_tokens: 200,
        model: String::new(),
    };
    let deps = build_deps(provider, compact_config, model);

    let mut builder = ContextBuilder::new(4096);
    for prompt in ["q1", "q2", "q3", "q4"] {
        let mut loop_state = AgentLoop {
            deps: deps.clone(),
            builder: std::mem::replace(&mut builder, ContextBuilder::new(0)),
        };
        loop_state.run(prompt).await.unwrap();
        builder = loop_state.builder;
        assert!(
            validate_no_orphans(&builder.messages).is_ok(),
            "validation failed for prompt {prompt}"
        );
    }
    let has_summary = builder
        .messages
        .iter()
        .any(|m| m.content.contains("=== compacted"));
    assert!(has_summary, "expected a compacted summary to be present");
}

/// Verify that calling compact() manually on a short conversation (fewer user
/// turns than keep_last_user_turns) results in a Noop strategy and no messages
/// are removed.
#[tokio::test]
async fn manual_compact_runs_truncate_only_on_short_convo() {
    let model = Arc::new(RwLock::new("openai:gpt-4o-mini".to_string()));
    let provider: Arc<dyn LlmProvider> = Arc::new(ScriptedProvider {
        responses: Mutex::new(vec![stop("only reply", 200)]),
        model: model.clone(),
    });
    let compact_config = CompactConfig {
        auto_threshold_pct: 0,
        keep_last_user_turns: 4,
        tool_output_max_bytes: 2048,
        summary_max_tokens: 200,
        model: String::new(),
    };
    let deps = build_deps(provider, compact_config.clone(), model.clone());
    let mut builder = ContextBuilder::new(4096);
    {
        let mut loop_state = AgentLoop {
            deps: deps.clone(),
            builder: std::mem::replace(&mut builder, ContextBuilder::new(0)),
        };
        loop_state.run("hi").await.unwrap();
        builder = loop_state.builder;
    }
    assert!(validate_no_orphans(&builder.messages).is_ok());

    let report = llmsh_core::compactor::compact(
        &mut builder.messages,
        &compact_config,
        llmsh_core::compactor::CompactionReason::Manual,
        "openai:gpt-4o-mini",
        u32::MAX / 2,
        deps.provider.clone(),
    )
    .await;
    assert_eq!(
        report.strategy,
        llmsh_core::compactor::CompactionStrategy::Noop
    );
    assert_eq!(report.messages_before, report.messages_after);
}
