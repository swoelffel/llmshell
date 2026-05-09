mod common;

use common::build_test_deps_with_memory;
use llmsh_core::agent::AgentLoop;
use llmsh_core::confirm::AlwaysYesGate;
use llmsh_core::context::ContextBuilder;
use llmsh_core::memory::Memory;
use llmsh_llm::types::{FinishReason, LlmResponse, Message, MessageRole};
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

fn stop_response(text: &str) -> LlmResponse {
    LlmResponse {
        message: Some(text.into()),
        tool_calls: vec![],
        finish_reason: FinishReason::Stop,
        usage: None,
    }
}

/// Build deps, run a turn, then drop and rebuild with the same DB path.
/// The second agent's first request must include the messages from the first
/// session (user input + assistant reply) when the caller reloads them from
/// the DB and passes them to ContextBuilder::with_messages — simulating what
/// main.rs does at startup.
#[tokio::test]
async fn actions_persist_across_sessions() {
    let tmp_work = tempfile::tempdir().unwrap();
    let tmp_db = tempfile::tempdir().unwrap();
    let db_path = tmp_db.path().join("memory.db");

    // Session 1
    {
        let memory = Arc::new(Memory::open(&db_path).unwrap());
        let audit_dir = tempfile::tempdir().unwrap();
        let (deps, _) = build_test_deps_with_memory(
            Arc::new(ToolRegistry::new()),
            vec![stop_response("session one response")],
            Arc::new(AlwaysYesGate),
            audit_dir.path(),
            policy_ctx_for(tmp_work.path()),
            vec![],
            None,
            memory,
        );
        let mut agent = AgentLoop {
            deps,
            builder: ContextBuilder::new(4096),
        };
        agent.run("session one input").await.unwrap();
    }

    // Session 2 — fresh agent, same DB.
    // Simulate what main.rs does: reload conversation from SQLite and inject
    // into the context builder via ContextBuilder::with_messages.
    {
        let memory = Arc::new(Memory::open(&db_path).unwrap());

        // Reload persisted messages (equivalent to main.rs startup hydration).
        let initial_messages: Vec<Message> = memory
            .load_active_conversation()
            .unwrap()
            .into_iter()
            .map(|r| Message {
                role: match r.role.as_str() {
                    "user" => MessageRole::User,
                    "assistant" => MessageRole::Assistant,
                    "tool" => MessageRole::Tool,
                    _ => MessageRole::User,
                },
                content: r.content,
                tool_call_id: r.tool_call_id,
                name: r.name,
                tool_calls: r
                    .tool_calls_json
                    .and_then(|s| serde_json::from_str(&s).ok()),
            })
            .collect();

        // Session 1 must have persisted exactly 2 messages: user + assistant.
        assert_eq!(
            initial_messages.len(),
            2,
            "DB must hold 2 messages from session 1"
        );
        assert_eq!(initial_messages[0].role, MessageRole::User);
        assert!(
            initial_messages[0].content.contains("session one input"),
            "user message content mismatch: {}",
            initial_messages[0].content
        );
        assert_eq!(initial_messages[1].role, MessageRole::Assistant);
        assert!(
            initial_messages[1].content.contains("session one response"),
            "assistant message content mismatch: {}",
            initial_messages[1].content
        );

        let audit_dir = tempfile::tempdir().unwrap();
        let (deps, provider) = build_test_deps_with_memory(
            Arc::new(ToolRegistry::new()),
            vec![stop_response("ok")],
            Arc::new(AlwaysYesGate),
            audit_dir.path(),
            policy_ctx_for(tmp_work.path()),
            vec![],
            None,
            memory,
        );
        let mut agent = AgentLoop {
            deps,
            builder: ContextBuilder::with_messages(4096, initial_messages),
        };
        agent.run("session two input").await.unwrap();

        // The LLM request for session 2 must contain the hydrated messages PLUS
        // the new user turn: [user(s1), assistant(s1), user(s2)].
        let captured = provider.captured.lock().unwrap();
        assert!(!captured.is_empty());
        let msgs = &captured[0].messages;
        assert_eq!(
            msgs.len(),
            3,
            "session 2 turn 1 must have 3 messages (s1 user + s1 assistant + s2 user)"
        );
        assert!(
            msgs[0].content.contains("session one input"),
            "first message must be session-1 user input"
        );
        assert!(
            msgs[1].content.contains("session one response"),
            "second message must be session-1 assistant reply"
        );
        assert!(
            msgs[2].content.contains("session two input"),
            "third message must be session-2 user input"
        );
    }
}
