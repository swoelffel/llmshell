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

const FAKE_OPENAI_KEY: &str = "sk-AAAAAAAAAAAAAAAAAAAAAAAA";

fn policy_ctx_for(cwd: &std::path::Path) -> PolicyContext {
    let canonical = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    PolicyContext {
        cwd: std::sync::Arc::new(std::sync::RwLock::new(canonical.clone())),
        workspace_root: canonical.clone(),
        allowed_roots: vec![canonical],
        sensitive_path_patterns: vec![],
    }
}

/// User input containing a secret must be redacted before being stored in
/// memory; the next turn's system prompt must NOT contain the literal secret.
#[tokio::test]
async fn user_input_redacted_before_memory_storage() {
    let tmp = tempfile::tempdir().unwrap();
    let audit_dir = tempfile::tempdir().unwrap();

    let memory = Arc::new(Memory::open_in_memory().unwrap());

    let responses = vec![
        LlmResponse {
            message: Some("ok".into()),
            tool_calls: vec![],
            finish_reason: FinishReason::Stop,
            usage: None,
        },
        LlmResponse {
            message: Some("ok again".into()),
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

    let user_msg = format!("my key is {} please remember", FAKE_OPENAI_KEY);
    agent.run(&user_msg).await.unwrap();
    agent.run("again").await.unwrap();

    let captured = provider.captured.lock().unwrap();
    // Turn 2 must have seen the user message from turn 1 in context (conversation history).
    assert_eq!(
        captured[1].messages.len(),
        3, // user(t1) + assistant(t1) + user(t2)
        "turn 2 should carry conversation history from turn 1"
    );
    // The secret must not appear in the system prompt of turn 2.
    let system_turn2 = captured[1].system.as_deref().unwrap_or("");
    assert!(
        !system_turn2.contains(FAKE_OPENAI_KEY),
        "turn 2 system prompt must NOT contain literal OpenAI key, got: {}",
        system_turn2
    );
    // The messages themselves should contain the redacted form, not the literal key.
    let messages_json = serde_json::to_string(&captured[1].messages).unwrap_or_default();
    assert!(
        !messages_json.contains(FAKE_OPENAI_KEY),
        "turn 2 messages must NOT contain literal OpenAI key after LLM redaction"
    );
}

/// Assistant text containing a secret must be redacted before being stored
/// in memory.
#[tokio::test]
async fn assistant_text_redacted_before_memory_storage() {
    let tmp = tempfile::tempdir().unwrap();
    let audit_dir = tempfile::tempdir().unwrap();

    let memory = Arc::new(Memory::open_in_memory().unwrap());

    let leaky_reply = format!("Sure, here it is: {}", FAKE_OPENAI_KEY);
    let responses = vec![
        LlmResponse {
            message: Some(leaky_reply),
            tool_calls: vec![],
            finish_reason: FinishReason::Stop,
            usage: None,
        },
        LlmResponse {
            message: Some("ok".into()),
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

    agent.run("show me the key").await.unwrap();
    agent.run("thanks").await.unwrap();

    let captured = provider.captured.lock().unwrap();
    let system_turn2 = captured[1].system.as_deref().unwrap_or("");
    assert!(
        !system_turn2.contains(FAKE_OPENAI_KEY),
        "assistant secret must not leak into next-turn system prompt: {}",
        system_turn2
    );
}
