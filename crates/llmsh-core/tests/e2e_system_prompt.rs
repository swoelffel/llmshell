mod common;

use common::build_test_deps_with_agents_md;
use llmsh_core::agent::AgentLoop;
use llmsh_core::confirm::AlwaysYesGate;
use llmsh_core::context::{ContextBuilder, DEFAULT_PERSONA};
use llmsh_llm::types::{FinishReason, LlmResponse};
use llmsh_policy::context::PolicyContext;
use llmsh_tools::registry::ToolRegistry;
use std::sync::Arc;

fn single_stop_response() -> Vec<LlmResponse> {
    vec![LlmResponse {
        message: Some("ok".into()),
        tool_calls: vec![],
        finish_reason: FinishReason::Stop,
        usage: None,
    }]
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

/// LlmRequest.system starts with the persona, no AGENTS.md block when absent.
#[tokio::test]
async fn system_starts_with_persona_no_agents_md() {
    let tmp = tempfile::tempdir().unwrap();
    let audit_dir = tempfile::tempdir().unwrap();

    let (deps, provider) = build_test_deps_with_agents_md(
        Arc::new(ToolRegistry::new()),
        single_stop_response(),
        Arc::new(AlwaysYesGate),
        audit_dir.path(),
        policy_ctx_for(tmp.path()),
        vec![],
        None,
    );

    let mut agent = AgentLoop {
        deps,
        builder: ContextBuilder::new(4096),
    };
    agent.run("hello").await.unwrap();

    let captured = provider.captured.lock().unwrap();
    assert!(
        !captured.is_empty(),
        "provider should have received a request"
    );
    let system = captured[0].system.as_deref().unwrap_or("");
    assert!(
        system.starts_with(DEFAULT_PERSONA),
        "system prompt must start with persona"
    );
    assert!(
        !system.contains("=== AGENTS.md ==="),
        "system prompt must not contain AGENTS.md block when none provided"
    );
}

/// When AGENTS.md content is provided, the system prompt contains the header and content.
#[tokio::test]
async fn system_contains_agents_md_when_present() {
    let tmp = tempfile::tempdir().unwrap();
    let audit_dir = tempfile::tempdir().unwrap();

    let agents_md_content = "Be terse and factual.";
    let (deps, provider) = build_test_deps_with_agents_md(
        Arc::new(ToolRegistry::new()),
        single_stop_response(),
        Arc::new(AlwaysYesGate),
        audit_dir.path(),
        policy_ctx_for(tmp.path()),
        vec![],
        Some(agents_md_content.to_string()),
    );

    let mut agent = AgentLoop {
        deps,
        builder: ContextBuilder::new(4096),
    };
    agent.run("hello").await.unwrap();

    let captured = provider.captured.lock().unwrap();
    let system = captured[0].system.as_deref().unwrap_or("");
    assert!(
        system.starts_with(DEFAULT_PERSONA),
        "system prompt must still start with persona"
    );
    assert!(
        system.contains("=== AGENTS.md ==="),
        "system prompt must contain AGENTS.md header"
    );
    assert!(
        system.contains(agents_md_content),
        "system prompt must contain AGENTS.md body"
    );
}

/// AGENTS.md block appears after the persona (persona is the stable prefix).
#[tokio::test]
async fn agents_md_comes_after_persona() {
    let tmp = tempfile::tempdir().unwrap();
    let audit_dir = tempfile::tempdir().unwrap();

    let (deps, provider) = build_test_deps_with_agents_md(
        Arc::new(ToolRegistry::new()),
        single_stop_response(),
        Arc::new(AlwaysYesGate),
        audit_dir.path(),
        policy_ctx_for(tmp.path()),
        vec![],
        Some("some content".to_string()),
    );

    let mut agent = AgentLoop {
        deps,
        builder: ContextBuilder::new(4096),
    };
    agent.run("hello").await.unwrap();

    let captured = provider.captured.lock().unwrap();
    let system = captured[0].system.as_deref().unwrap_or("");
    let persona_pos = system.find(DEFAULT_PERSONA).expect("persona not found");
    let agents_pos = system
        .find("=== AGENTS.md ===")
        .expect("AGENTS.md not found");
    assert!(
        persona_pos < agents_pos,
        "persona must appear before AGENTS.md block"
    );
}
