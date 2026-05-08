mod common;

use common::build_simple_deps;
use llmsh_core::agent::AgentLoop;
use llmsh_core::context::ContextBuilder;
use llmsh_llm::types::{FinishReason, LlmResponse, ToolCall};
use llmsh_tools::list_directory::ListDirectory;
use llmsh_tools::registry::ToolRegistry;
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn list_directory_happy_path() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("a.txt"), "x").unwrap();

    let scripted = vec![
        LlmResponse {
            message: None,
            tool_calls: vec![ToolCall {
                id: "c1".into(),
                name: "list_directory".into(),
                args: json!({"path": "."}),
            }],
            finish_reason: FinishReason::ToolCalls,
            usage: None,
        },
        LlmResponse {
            message: Some("Listed directory".into()),
            tool_calls: vec![],
            finish_reason: FinishReason::Stop,
            usage: None,
        },
    ];

    let mut reg = ToolRegistry::new();
    reg.register(Arc::new(ListDirectory));
    let registry = Arc::new(reg);

    let audit_dir = tempfile::tempdir().unwrap();
    let deps = build_simple_deps(registry, scripted, tmp.path(), audit_dir.path());

    let mut agent = AgentLoop {
        deps: deps.clone(),
        builder: ContextBuilder::new(4096),
    };
    let res = agent.run("liste les fichiers ici").await.unwrap();
    assert_eq!(res.assistant_text.as_deref(), Some("Listed directory"));

    deps.audit.lock().unwrap().flush().unwrap();
    let log = std::fs::read_to_string(audit_dir.path().join("test-session.jsonl")).unwrap();
    assert!(
        log.contains("\"type\":\"llm_request\""),
        "missing llm_request"
    );
    assert!(
        log.contains("\"type\":\"tool_execution_start\""),
        "missing tool_execution_start"
    );
    assert!(
        log.contains("\"type\":\"tool_execution_end\""),
        "missing tool_execution_end"
    );
    assert!(
        log.contains("\"type\":\"assistant_message\""),
        "missing assistant_message"
    );
}
