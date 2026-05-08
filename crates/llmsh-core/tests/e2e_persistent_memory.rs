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

fn stop_response(text: &str) -> LlmResponse {
    LlmResponse {
        message: Some(text.into()),
        tool_calls: vec![],
        finish_reason: FinishReason::Stop,
        usage: None,
    }
}

/// Build deps, run a turn, then drop and rebuild with the same DB path.
/// The second agent's first request must contain "=== Recent activity ===" with
/// the user input and assistant reply from the first session.
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

    // Session 2 — fresh agent, same DB
    {
        let memory = Arc::new(Memory::open(&db_path).unwrap());
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
            builder: ContextBuilder::new(4096),
        };
        agent.run("session two input").await.unwrap();

        let captured = provider.captured.lock().unwrap();
        assert!(!captured.is_empty());
        let system = captured[0].system.as_deref().unwrap_or("");
        assert!(
            system.contains("=== Recent activity ==="),
            "second session must see Recent activity section; got: {}",
            system
        );
        assert!(
            system.contains("session one input"),
            "second session must see user input from first session; got: {}",
            system
        );
        assert!(
            system.contains("session one response"),
            "second session must see assistant reply from first session; got: {}",
            system
        );
    }
}
