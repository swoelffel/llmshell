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
        cwd: std::sync::Arc::new(std::sync::RwLock::new(canonical.clone())),
        workspace_root: canonical.clone(),
        allowed_roots: vec![canonical],
        sensitive_path_patterns: vec![],
    }
}

/// Turn 1: user says "hello", model returns "hi there".
/// Turn 2: the LLM request must include the messages from turn 1 in its
/// messages list so the model sees the full conversation context.
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

    // Turn 2 must carry [user(t1), assistant(t1), user(t2)] in its messages.
    let msgs_turn2 = &captured[1].messages;
    assert_eq!(
        msgs_turn2.len(),
        3,
        "turn 2 must have 3 messages: user(t1), assistant(t1), user(t2)"
    );
    assert!(
        msgs_turn2[0].content.contains("hello"),
        "turn 2 messages[0] must be user input from turn 1; got: {}",
        msgs_turn2[0].content
    );
    assert!(
        msgs_turn2[1].content.contains("hi there"),
        "turn 2 messages[1] must be assistant reply from turn 1; got: {}",
        msgs_turn2[1].content
    );
    assert!(
        msgs_turn2[2].content.contains("what did you just say"),
        "turn 2 messages[2] must be user input from turn 2; got: {}",
        msgs_turn2[2].content
    );
}
