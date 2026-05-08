mod common;

use llmsh_core::agent::AgentLoop;
use llmsh_core::confirm::AlwaysYesGate;
use llmsh_core::context::{ContextBuilder, MemorySystemPrompt};
use llmsh_core::memory::Memory;
use llmsh_llm::types::{FinishReason, LlmResponse};
use llmsh_policy::context::PolicyContext;
use llmsh_tools::registry::ToolRegistry;
use std::sync::Arc;
use std::time::Instant;

fn single_stop_response() -> Vec<LlmResponse> {
    vec![LlmResponse {
        message: Some("ok".into()),
        tool_calls: vec![],
        finish_reason: FinishReason::Stop,
        usage: None,
    }]
}

#[tokio::test]
async fn system_prompt_contains_runtime_context_block() {
    let workspace = tempfile::tempdir().unwrap();
    let audit_dir = tempfile::tempdir().unwrap();
    let model = Arc::new("mock:test-model".to_string());
    let session_start = Instant::now();

    let canonical_ws =
        std::fs::canonicalize(workspace.path()).unwrap_or_else(|_| workspace.path().to_path_buf());
    let policy_ctx = PolicyContext {
        cwd: canonical_ws.clone(),
        workspace_root: canonical_ws.clone(),
        allowed_roots: vec![canonical_ws.clone()],
        sensitive_path_patterns: vec![],
    };

    let memory = Arc::new(Memory::open_in_memory().unwrap());
    let system_prompt = Arc::new(MemorySystemPrompt::new(
        None,
        memory.clone(),
        canonical_ws.clone(),
        model.clone(),
        session_start,
    ));

    let registry = ToolRegistry::new();
    let (deps, provider) = {
        use llmsh_audit::redact::Redactor;
        use llmsh_audit::writer::AuditWriter;
        use llmsh_core::agent::{AgentBounds, AgentDeps};
        use llmsh_core::executor::ToolExecutor;
        use llmsh_core::pipeline::Pipeline;
        use llmsh_policy::engine::{DefaultPolicyConfig, DefaultPolicyEngine};
        use std::sync::Mutex;
        use tokio_util::sync::CancellationToken;

        let registry = Arc::new(registry);
        let provider = Arc::new(common::MockLlmProvider::new(single_stop_response()));
        let policy = Arc::new(DefaultPolicyEngine::new(DefaultPolicyConfig::default()));
        let pipeline = Pipeline {
            registry: registry.clone(),
            policy,
            home: None,
        };
        let writer = AuditWriter::open(audit_dir.path(), "test-session").unwrap();
        let deps = Arc::new(AgentDeps {
            provider: provider.clone(),
            pipeline,
            executor: ToolExecutor {
                registry,
                timeout: std::time::Duration::from_secs(5),
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
            policy_ctx,
            sensitive_patterns: vec![],
            model_label: "mock:test-model".into(),
            system_prompt,
            memory,
        });
        (deps, provider)
    };

    let mut agent = AgentLoop {
        deps,
        builder: ContextBuilder::new(4096),
    };
    agent.run("hello").await.unwrap();

    let captured = provider.captured.lock().unwrap();
    assert!(!captured.is_empty());
    let system = captured[0].system.as_deref().unwrap_or("");

    assert!(
        system.contains("=== Runtime context ==="),
        "system prompt must contain runtime context header"
    );

    let rt_start = system
        .find("=== Runtime context ===")
        .expect("runtime context header");
    let rt_body = &system[rt_start..];

    assert!(
        rt_body.contains("host:"),
        "runtime context must contain host line"
    );
    assert!(
        rt_body.contains("user:"),
        "runtime context must contain user line"
    );
    assert!(
        rt_body.contains("cwd:"),
        "runtime context must contain cwd line"
    );
    assert!(
        rt_body.contains("workspace_root:"),
        "runtime context must contain workspace_root line"
    );
    assert!(
        rt_body.contains("model:"),
        "runtime context must contain model line"
    );
    assert!(
        rt_body.contains("session uptime:"),
        "runtime context must contain session uptime line"
    );

    let expected_ws = canonical_ws.display().to_string();
    assert!(
        rt_body.contains(&format!("workspace_root: {}", expected_ws)),
        "workspace_root must match the passed-in value"
    );
    assert!(
        rt_body.contains("model: mock:test-model"),
        "model must match the passed-in value"
    );
}
