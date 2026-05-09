mod common;

use common::build_test_deps;
use llmsh_core::agent::AgentLoop;
use llmsh_core::confirm::AlwaysNoGate;
use llmsh_core::context::ContextBuilder;
use llmsh_llm::types::{FinishReason, LlmResponse, ToolCall};
use llmsh_policy::context::PolicyContext;
use llmsh_tools::registry::ToolRegistry;
use llmsh_tools::run_process::RunProcess;
use serde_json::json;
use std::sync::Arc;

/// `rm -rf ./tmp-test` with `AlwaysNoGate` must be cancelled before execution.
///
/// Expected audit events:
/// - `confirmation_asked` with `granted: false`
/// - NO `tool_execution_start`
#[tokio::test]
async fn destructive_rm_rf_cancelled_by_gate() {
    let tmp = tempfile::tempdir().unwrap();

    let scripted = vec![LlmResponse {
        message: None,
        tool_calls: vec![ToolCall {
            id: "c1".into(),
            name: "run_process".into(),
            args: json!({"program": "rm", "args": ["-rf", "./tmp-test"]}),
        }],
        finish_reason: FinishReason::ToolCalls,
        usage: None,
    }];

    let mut reg = ToolRegistry::new();
    reg.register(Arc::new(RunProcess));
    let registry = Arc::new(reg);

    let audit_dir = tempfile::tempdir().unwrap();

    let policy_ctx = PolicyContext {
        cwd: std::sync::Arc::new(std::sync::RwLock::new(tmp.path().to_path_buf())),
        workspace_root: tmp.path().to_path_buf(),
        allowed_roots: vec![tmp.path().to_path_buf()],
        sensitive_path_patterns: vec![],
    };

    let deps = build_test_deps(
        registry,
        scripted,
        Arc::new(AlwaysNoGate),
        audit_dir.path(),
        policy_ctx,
        vec![],
    );

    let mut agent = AgentLoop {
        deps: deps.clone(),
        builder: ContextBuilder::new(4096),
    };
    let res = agent.run("supprime le dossier tmp-test").await.unwrap();
    assert_eq!(
        res.stopped_reason, "cancelled",
        "loop should stop with reason=cancelled"
    );

    deps.audit.lock().unwrap().flush().unwrap();
    let log = std::fs::read_to_string(audit_dir.path().join("test-session.jsonl")).unwrap();

    assert!(
        log.contains("\"type\":\"confirmation_asked\""),
        "expected confirmation_asked in audit log"
    );
    assert!(
        log.contains("\"granted\":false"),
        "expected granted=false in audit log"
    );
    assert!(
        !log.contains("\"type\":\"tool_execution_start\""),
        "tool_execution_start must NOT appear when confirmation was refused"
    );
}
