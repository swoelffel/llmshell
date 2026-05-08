//! Verifies that a ContextBuilder reused across two AgentLoop turns accumulates
//! all user/assistant messages so the LLM sees full conversation history.

mod common;

use llmsh_core::agent::AgentLoop;
use llmsh_core::context::ContextBuilder;
use llmsh_llm::types::{FinishReason, LlmResponse};
use llmsh_tools::registry::ToolRegistry;
use std::sync::Arc;

/// Run two agent turns sharing the same ContextBuilder via mem::replace.
/// After both turns the builder must hold 4 messages: user1, assistant1, user2, assistant2.
#[tokio::test]
async fn conversation_history_persists_across_turns() {
    let audit_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();

    let responses = vec![
        LlmResponse {
            message: Some("first reply".into()),
            tool_calls: vec![],
            finish_reason: FinishReason::Stop,
            usage: None,
        },
        LlmResponse {
            message: Some("second reply".into()),
            tool_calls: vec![],
            finish_reason: FinishReason::Stop,
            usage: None,
        },
    ];

    let registry = Arc::new(ToolRegistry::new());
    let deps = common::build_simple_deps(registry, responses, cwd.path(), audit_dir.path());

    let mut builder = ContextBuilder::new(4096);

    // Turn 1
    {
        let mut loop1 = AgentLoop {
            deps: deps.clone(),
            builder: std::mem::replace(&mut builder, ContextBuilder::new(0)),
        };
        loop1.run("hello").await.unwrap();
        builder = loop1.builder;
    }

    // Turn 2
    {
        let mut loop2 = AgentLoop {
            deps: deps.clone(),
            builder: std::mem::replace(&mut builder, ContextBuilder::new(0)),
        };
        loop2.run("again").await.unwrap();
        builder = loop2.builder;
    }

    // After two turns the builder must hold 4 messages in order.
    assert_eq!(
        builder.messages.len(),
        4,
        "expected 4 messages (user1, assistant1, user2, assistant2)"
    );

    assert_eq!(
        builder.messages[0].role,
        llmsh_llm::types::MessageRole::User
    );
    assert_eq!(builder.messages[0].content, "hello");

    assert_eq!(
        builder.messages[1].role,
        llmsh_llm::types::MessageRole::Assistant
    );
    assert_eq!(builder.messages[1].content, "first reply");

    assert_eq!(
        builder.messages[2].role,
        llmsh_llm::types::MessageRole::User
    );
    assert_eq!(builder.messages[2].content, "again");

    assert_eq!(
        builder.messages[3].role,
        llmsh_llm::types::MessageRole::Assistant
    );
    assert_eq!(builder.messages[3].content, "second reply");
}
