mod common;

use common::build_test_deps;
use llmsh_core::agent::AgentLoop;
use llmsh_core::confirm::AlwaysYesGate;
use llmsh_core::context::ContextBuilder;
use llmsh_llm::types::{FinishReason, LlmResponse, ToolCall};
use llmsh_policy::context::PolicyContext;
use llmsh_tools::read_file::ReadFile;
use llmsh_tools::registry::ToolRegistry;
use serde_json::json;
use std::sync::Arc;

/// `read_file ~/.ssh/id_rsa` must be denied by the sensitive-path policy.
/// The audit log must contain `"kind":"deny"` and must NOT contain a
/// `tool_execution_start` event.
#[tokio::test]
async fn sensitive_path_denied() {
    let tmp = tempfile::tempdir().unwrap();

    // The sensitive pattern covers anything under ~/.ssh
    let home = tmp.path().join("home").join("testuser");
    std::fs::create_dir_all(home.join(".ssh")).unwrap();
    std::fs::write(home.join(".ssh").join("id_rsa"), "fake key").unwrap();

    let scripted = vec![
        // Mock returns a single read_file call to the sensitive path.
        // The agent loop will deny it and return immediately.
        LlmResponse {
            message: None,
            tool_calls: vec![ToolCall {
                id: "c1".into(),
                name: "read_file".into(),
                args: json!({"path": home.join(".ssh/id_rsa").to_str().unwrap()}),
            }],
            finish_reason: FinishReason::ToolCalls,
            usage: None,
        },
    ];

    let mut reg = ToolRegistry::new();
    reg.register(Arc::new(ReadFile));
    let registry = Arc::new(reg);

    let audit_dir = tempfile::tempdir().unwrap();
    let sensitive_patterns = vec![
        format!("{}/.ssh/**", home.display()),
        "**/id_rsa".to_string(),
    ];

    let policy_ctx = PolicyContext {
        cwd: tmp.path().to_path_buf(),
        workspace_root: tmp.path().to_path_buf(),
        allowed_roots: vec![tmp.path().to_path_buf(), home.clone()],
        sensitive_path_patterns: sensitive_patterns.clone(),
    };

    let deps = build_test_deps(
        registry,
        scripted,
        Arc::new(AlwaysYesGate),
        audit_dir.path(),
        policy_ctx,
        sensitive_patterns,
    );

    let mut agent = AgentLoop {
        deps: deps.clone(),
        builder: ContextBuilder::new(4096),
    };
    let res = agent.run("lis le fichier ssh").await.unwrap();
    assert_eq!(res.stopped_reason, "denied");

    deps.audit.lock().unwrap().flush().unwrap();
    let log = std::fs::read_to_string(audit_dir.path().join("test-session.jsonl")).unwrap();

    assert!(
        log.contains("\"kind\":\"deny\""),
        "expected deny action in audit log"
    );
    assert!(
        !log.contains("\"type\":\"tool_execution_start\""),
        "tool_execution_start must NOT appear when path is denied"
    );
}

/// Requesting a tool that is NOT registered in the registry must produce a deny
/// decision with a "tool not in registry" reason.  No `tool_execution_start`
/// event may appear.
#[tokio::test]
async fn unknown_tool_denied() {
    let tmp = tempfile::tempdir().unwrap();

    let scripted = vec![LlmResponse {
        message: None,
        tool_calls: vec![ToolCall {
            id: "c1".into(),
            name: "search_files".into(), // not registered
            args: json!({"query": "secret"}),
        }],
        finish_reason: FinishReason::ToolCalls,
        usage: None,
    }];

    // Empty registry — no tools registered at all.
    let registry = Arc::new(ToolRegistry::new());
    let audit_dir = tempfile::tempdir().unwrap();

    let policy_ctx = PolicyContext {
        cwd: tmp.path().to_path_buf(),
        workspace_root: tmp.path().to_path_buf(),
        allowed_roots: vec![tmp.path().to_path_buf()],
        sensitive_path_patterns: vec![],
    };

    let deps = build_test_deps(
        registry,
        scripted,
        Arc::new(AlwaysYesGate),
        audit_dir.path(),
        policy_ctx,
        vec![],
    );

    let mut agent = AgentLoop {
        deps: deps.clone(),
        builder: ContextBuilder::new(4096),
    };
    let res = agent.run("cherche des fichiers").await.unwrap();
    assert_eq!(res.stopped_reason, "denied");

    deps.audit.lock().unwrap().flush().unwrap();
    let log = std::fs::read_to_string(audit_dir.path().join("test-session.jsonl")).unwrap();

    assert!(
        log.contains("tool not in registry"),
        "expected 'tool not in registry' reason in audit log"
    );
    assert!(
        !log.contains("\"type\":\"tool_execution_start\""),
        "tool_execution_start must NOT appear for unknown tool"
    );
}
