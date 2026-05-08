mod common;

use common::build_test_deps_with_memory;
use llmsh_core::agent::AgentLoop;
use llmsh_core::confirm::AlwaysYesGate;
use llmsh_core::context::ContextBuilder;
use llmsh_core::init::MachineAudit;
use llmsh_core::memory::Memory;
use llmsh_llm::types::{FinishReason, LlmResponse};
use llmsh_policy::context::PolicyContext;
use llmsh_tools::registry::ToolRegistry;
use std::sync::Arc;

fn stop_response(text: &str) -> LlmResponse {
    LlmResponse {
        message: Some(text.into()),
        tool_calls: vec![],
        finish_reason: FinishReason::Stop,
        usage: None,
    }
}

fn policy_ctx_for(cwd: &std::path::Path) -> PolicyContext {
    let canonical = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    PolicyContext {
        cwd: canonical.clone(),
        workspace_root: canonical.clone(),
        allowed_roots: vec![canonical],
        sensitive_path_patterns: vec![],
    }
}

/// Write an init audit and confirm it round-trips via read_init_audit.
#[tokio::test]
async fn init_audit_written_and_readable() {
    let tmp_db = tempfile::tempdir().unwrap();
    let db_path = tmp_db.path().join("memory.db");
    let memory = Arc::new(Memory::open(&db_path).unwrap());

    assert!(memory.read_init_audit().unwrap().is_none());

    let audit = MachineAudit::capture_with_tooling().await;
    memory.write_init_audit(&audit.into_init_audit()).unwrap();

    assert!(memory.read_init_audit().unwrap().is_some());
}

/// After writing an init audit, the system prompt must contain the long-term memory section
/// with the markdown.
#[tokio::test]
async fn system_prompt_contains_long_term_memory_after_init() {
    let tmp_work = tempfile::tempdir().unwrap();
    let tmp_db = tempfile::tempdir().unwrap();
    let db_path = tmp_db.path().join("memory.db");
    let memory = Arc::new(Memory::open(&db_path).unwrap());

    // Write an init audit directly.
    let audit = MachineAudit::capture_with_tooling().await;
    let md = audit.render_markdown();
    memory.write_init_audit(&audit.into_init_audit()).unwrap();

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
    agent.run("hello").await.unwrap();

    let captured = provider.captured.lock().unwrap();
    assert!(!captured.is_empty());
    let system = captured[0].system.as_deref().unwrap_or("");
    assert!(
        system.contains("=== Long-term memory ==="),
        "system prompt must contain Long-term memory section; got:\n{}",
        system
    );
    assert!(
        system.contains("# Machine audit"),
        "long-term memory must contain the markdown audit header; got:\n{}",
        system
    );
    // The summary_md should include some identity info from the real machine.
    assert!(
        system.contains(&md[..md.len().min(50)]),
        "system prompt should contain beginning of the written markdown"
    );
}
