mod common;

use common::build_simple_deps;
use llmsh_core::agent::AgentLoop;
use llmsh_core::context::ContextBuilder;
use llmsh_llm::types::{FinishReason, LlmResponse, ToolCall};
use llmsh_tools::list_directory::ListDirectory;
use llmsh_tools::registry::ToolRegistry;
use serde_json::json;
use std::sync::Arc;

/// The mock LLM first returns a tool call with missing required args (`path` is
/// absent).  The pipeline detects the schema error and feeds it back as a tool
/// result.  On the second iteration the mock returns correct args and the agent
/// succeeds.
#[tokio::test]
async fn schema_repair_succeeds_on_second_attempt() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("hello.txt"), "world").unwrap();

    let scripted = vec![
        // First call: bad args — `path` is missing.
        LlmResponse {
            message: None,
            tool_calls: vec![ToolCall {
                id: "c1".into(),
                name: "list_directory".into(),
                args: json!({}), // missing required "path"
            }],
            finish_reason: FinishReason::ToolCalls,
            usage: None,
        },
        // Second call: correct args.
        LlmResponse {
            message: None,
            tool_calls: vec![ToolCall {
                id: "c2".into(),
                name: "list_directory".into(),
                args: json!({"path": "."}),
            }],
            finish_reason: FinishReason::ToolCalls,
            usage: None,
        },
        // Final stop.
        LlmResponse {
            message: Some("Repair succeeded".into()),
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
    let res = agent.run("liste les fichiers").await.unwrap();
    assert_eq!(
        res.assistant_text.as_deref(),
        Some("Repair succeeded"),
        "expected final assistant message after schema repair"
    );
    assert_eq!(res.stopped_reason, "stop");

    deps.audit.lock().unwrap().flush().unwrap();
    let log = std::fs::read_to_string(audit_dir.path().join("test-session.jsonl")).unwrap();

    // The repaired (second) call must have actually executed.
    assert!(
        log.contains("\"type\":\"tool_execution_start\""),
        "expected tool_execution_start after repair"
    );
    assert!(
        log.contains("\"type\":\"tool_execution_end\""),
        "expected tool_execution_end after repair"
    );
    assert!(
        log.contains("\"type\":\"assistant_message\""),
        "expected assistant_message after repair"
    );
}
