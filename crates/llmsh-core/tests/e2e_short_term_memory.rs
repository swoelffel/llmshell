mod common;

use common::build_test_deps_with_memory;
use llmsh_core::agent::AgentLoop;
use llmsh_core::confirm::AlwaysYesGate;
use llmsh_core::context::ContextBuilder;
use llmsh_core::memory::Memory;
use llmsh_llm::types::{FinishReason, LlmResponse};
use llmsh_policy::context::PolicyContext;
use llmsh_tools::registry::ToolRegistry;
use std::sync::Arc;

fn policy_ctx_for(cwd: &std::path::Path) -> PolicyContext {
    let canonical = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    PolicyContext {
        cwd: canonical.clone(),
        workspace_root: canonical.clone(),
        allowed_roots: vec![canonical],
        sensitive_path_patterns: vec![],
    }
}

/// Turn 1: user says "hello", model returns "hi there".
/// Turn 2: LlmRequest.system at turn 2 must contain "=== Recent activity ===" with
/// the user input and assistant reply from turn 1.
#[tokio::test]
async fn recent_activity_visible_in_second_turn() {
    let tmp = tempfile::tempdir().unwrap();
    let audit_dir = tempfile::tempdir().unwrap();

    let memory = Arc::new(Memory::open_in_memory().unwrap());

    let responses = vec![
        LlmResponse {
            message: Some("hi there".into()),
            tool_calls: vec![],
            finish_reason: FinishReason::Stop,
            usage: None,
        },
        LlmResponse {
            message: Some("i just said hi there".into()),
            tool_calls: vec![],
            finish_reason: FinishReason::Stop,
            usage: None,
        },
    ];

    let (deps, provider) = build_test_deps_with_memory(
        Arc::new(ToolRegistry::new()),
        responses,
        Arc::new(AlwaysYesGate),
        audit_dir.path(),
        policy_ctx_for(tmp.path()),
        vec![],
        None,
        memory,
    );

    let mut agent = AgentLoop {
        deps,
        builder: ContextBuilder::new(4096),
    };

    agent.run("hello").await.unwrap();
    agent.run("what did you just say?").await.unwrap();

    let captured = provider.captured.lock().unwrap();
    assert_eq!(captured.len(), 2, "expected 2 LLM requests");

    let system_turn2 = captured[1].system.as_deref().unwrap_or("");
    assert!(
        system_turn2.contains("=== Recent activity ==="),
        "turn 2 system prompt must contain Recent activity section"
    );
    assert!(
        system_turn2.contains("hello"),
        "turn 2 must reference user input from turn 1"
    );
    assert!(
        system_turn2.contains("hi there"),
        "turn 2 must reference assistant reply from turn 1"
    );
}
