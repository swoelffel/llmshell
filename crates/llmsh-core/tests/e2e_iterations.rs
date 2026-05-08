mod common;

use common::MockLlmProvider;
use llmsh_audit::redact::Redactor;
use llmsh_audit::writer::AuditWriter;
use llmsh_core::agent::{AgentBounds, AgentDeps, AgentLoop};
use llmsh_core::confirm::AlwaysYesGate;
use llmsh_core::context::{ContextBuilder, StaticSystemPrompt};
use llmsh_core::executor::ToolExecutor;
use llmsh_core::pipeline::Pipeline;
use llmsh_llm::types::{FinishReason, LlmResponse, ToolCall};
use llmsh_policy::context::PolicyContext;
use llmsh_policy::engine::{DefaultPolicyConfig, DefaultPolicyEngine};
use llmsh_tools::list_directory::ListDirectory;
use llmsh_tools::registry::ToolRegistry;
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// When the mock keeps returning `ToolCalls` and `max_iterations = 2`, the
/// agent must stop with `stopped_reason = "max_iterations"` and write an
/// `Error { code: "max_iterations" }` event to the audit log.
#[tokio::test]
async fn max_iterations_stops_loop() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("x.txt"), "data").unwrap();

    // Provide enough ToolCalls responses to fill the iterations without running dry.
    // With max_iterations=2 the loop fires the error on iter=3, so we need 2
    // successful ToolCall rounds before the check triggers.
    let tool_response = || LlmResponse {
        message: None,
        tool_calls: vec![ToolCall {
            id: "c1".into(),
            name: "list_directory".into(),
            args: json!({"path": "."}),
        }],
        finish_reason: FinishReason::ToolCalls,
        usage: None,
    };

    let scripted = vec![tool_response(), tool_response()];

    let mut reg = ToolRegistry::new();
    reg.register(Arc::new(ListDirectory));
    let registry = Arc::new(reg);

    let audit_dir = tempfile::tempdir().unwrap();
    let writer = AuditWriter::open(audit_dir.path(), "test-session").unwrap();
    let policy = Arc::new(DefaultPolicyEngine::new(DefaultPolicyConfig::default()));
    let pipeline = Pipeline {
        registry: registry.clone(),
        policy,
        home: None,
    };

    // Canonicalize to avoid macOS /var -> /private/var symlink mismatches.
    let canonical_tmp =
        std::fs::canonicalize(tmp.path()).unwrap_or_else(|_| tmp.path().to_path_buf());

    let deps = Arc::new(AgentDeps {
        provider: Arc::new(MockLlmProvider::new(scripted)),
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
            max_iterations: 2,
            max_tool_calls_per_iteration: 5,
            max_schema_repair_attempts: 2,
        },
        policy_ctx: PolicyContext {
            cwd: canonical_tmp.clone(),
            workspace_root: canonical_tmp.clone(),
            allowed_roots: vec![canonical_tmp],
            sensitive_path_patterns: vec![],
        },
        sensitive_patterns: vec![],
        model_label: "mock:test".into(),
        system_prompt: Arc::new(StaticSystemPrompt::new(None)),
    });

    let mut agent = AgentLoop {
        deps: deps.clone(),
        builder: ContextBuilder::new(4096),
    };
    let res = agent.run("loop forever").await.unwrap();
    assert_eq!(
        res.stopped_reason, "max_iterations",
        "expected loop to stop with max_iterations"
    );

    deps.audit.lock().unwrap().flush().unwrap();
    let log = std::fs::read_to_string(audit_dir.path().join("test-session.jsonl")).unwrap();

    assert!(
        log.contains("\"code\":\"max_iterations\""),
        "expected max_iterations error in audit log"
    );
    assert!(
        log.contains("\"type\":\"error\""),
        "expected error event in audit log"
    );
}
