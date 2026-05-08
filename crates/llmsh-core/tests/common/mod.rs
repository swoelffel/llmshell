use async_trait::async_trait;
use llmsh_audit::redact::Redactor;
use llmsh_audit::writer::AuditWriter;
use llmsh_core::agent::{AgentBounds, AgentDeps};
use llmsh_core::confirm::AlwaysYesGate;
use llmsh_core::executor::ToolExecutor;
use llmsh_core::memory::Memory;
use llmsh_core::pipeline::Pipeline;
use llmsh_llm::capabilities::{Capabilities, ToolCallingMode};
use llmsh_llm::provider::LlmProvider;
use llmsh_llm::types::{LlmRequest, LlmResponse};
use llmsh_policy::context::PolicyContext;
use llmsh_policy::engine::{DefaultPolicyConfig, DefaultPolicyEngine};
use llmsh_tools::registry::ToolRegistry;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

pub struct MockLlmProvider {
    pub scripted: Mutex<Vec<LlmResponse>>,
    pub captured: Mutex<Vec<LlmRequest>>,
}

impl MockLlmProvider {
    pub fn new(responses: Vec<LlmResponse>) -> Self {
        Self {
            scripted: Mutex::new(responses),
            captured: Mutex::new(vec![]),
        }
    }
}

#[async_trait]
impl LlmProvider for MockLlmProvider {
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
}

/// Build a standard `AgentDeps` for use in integration tests.
///
/// - `registry`: the tool registry (caller registers whatever tools are needed)
/// - `responses`: scripted LLM responses consumed in order
/// - `gate`: confirmation gate (use `AlwaysYesGate` or `AlwaysNoGate`)
/// - `audit_dir`: temp dir for the audit log
/// - `policy_ctx`: policy context
/// - `sensitive_patterns`: sensitive path patterns forwarded to the pipeline
#[allow(dead_code)]
pub fn build_test_deps(
    registry: Arc<ToolRegistry>,
    responses: Vec<LlmResponse>,
    gate: Arc<dyn llmsh_core::confirm::ConfirmationGate>,
    audit_dir: &std::path::Path,
    policy_ctx: PolicyContext,
    sensitive_patterns: Vec<String>,
) -> Arc<AgentDeps> {
    let (deps, _) = build_test_deps_with_agents_md(
        registry,
        responses,
        gate,
        audit_dir,
        policy_ctx,
        sensitive_patterns,
        None,
    );
    deps
}

/// Variant returning the `MockLlmProvider` so tests can inspect captured requests.
#[allow(dead_code)]
pub fn build_test_deps_with_agents_md(
    registry: Arc<ToolRegistry>,
    responses: Vec<LlmResponse>,
    gate: Arc<dyn llmsh_core::confirm::ConfirmationGate>,
    audit_dir: &std::path::Path,
    policy_ctx: PolicyContext,
    sensitive_patterns: Vec<String>,
    agents_md: Option<String>,
) -> (Arc<AgentDeps>, Arc<MockLlmProvider>) {
    build_test_deps_with_memory(
        registry,
        responses,
        gate,
        audit_dir,
        policy_ctx,
        sensitive_patterns,
        agents_md,
        Arc::new(Memory::open_in_memory().unwrap()),
    )
}

/// Full variant that also accepts a pre-opened `Memory` (for persistence tests).
#[allow(dead_code, clippy::too_many_arguments)]
pub fn build_test_deps_with_memory(
    registry: Arc<ToolRegistry>,
    responses: Vec<LlmResponse>,
    gate: Arc<dyn llmsh_core::confirm::ConfirmationGate>,
    audit_dir: &std::path::Path,
    policy_ctx: PolicyContext,
    sensitive_patterns: Vec<String>,
    agents_md: Option<String>,
    memory: Arc<Memory>,
) -> (Arc<AgentDeps>, Arc<MockLlmProvider>) {
    let provider = Arc::new(MockLlmProvider::new(responses));
    let policy = Arc::new(DefaultPolicyEngine::new(DefaultPolicyConfig::default()));
    let pipeline = Pipeline {
        registry: registry.clone(),
        policy,
        home: None,
    };
    let writer = AuditWriter::open(audit_dir, "test-session").unwrap();
    let system_prompt = Arc::new(llmsh_core::context::MemorySystemPrompt::new(
        agents_md,
        memory.clone(),
        std::env::temp_dir(),
        Arc::new("mock:test-model".to_string()),
        Instant::now(),
    ));
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
        gate,
        audit: Mutex::new(writer),
        redactor: Redactor::default_audit(),
        bounds: AgentBounds {
            max_iterations: 5,
            max_tool_calls_per_iteration: 5,
            max_schema_repair_attempts: 2,
        },
        policy_ctx,
        sensitive_patterns,
        model_label: "mock:test".into(),
        system_prompt,
        memory,
    });
    (deps, provider)
}

/// Convenience: build deps with `AlwaysYesGate` and no sensitive patterns, using
/// a temp dir as the workspace cwd.
///
/// The path is canonicalized before being stored so that macOS symlink aliases
/// (e.g. `/var` → `/private/var`) don't cause spurious `outside_workspace`
/// policy denials.
#[allow(dead_code)]
pub fn build_simple_deps(
    registry: Arc<ToolRegistry>,
    responses: Vec<LlmResponse>,
    cwd: &std::path::Path,
    audit_dir: &std::path::Path,
) -> Arc<AgentDeps> {
    let canonical = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    build_test_deps(
        registry,
        responses,
        Arc::new(AlwaysYesGate),
        audit_dir,
        PolicyContext {
            cwd: canonical.clone(),
            workspace_root: canonical.clone(),
            allowed_roots: vec![canonical],
            sensitive_path_patterns: vec![],
        },
        vec![],
    )
}
