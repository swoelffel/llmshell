mod common;

use common::MockLlmProvider;
use llmsh_audit::redact::Redactor;
use llmsh_audit::writer::AuditWriter;
use llmsh_core::agent::{AgentBounds, AgentDeps, AgentLoop};
use llmsh_core::confirm::AlwaysYesGate;
use llmsh_core::context::{ContextBuilder, StaticSystemPrompt, DEFAULT_PERSONA};
use llmsh_core::executor::ToolExecutor;
use llmsh_core::pipeline::Pipeline;
use llmsh_llm::types::{FinishReason, LlmResponse};
use llmsh_policy::context::PolicyContext;
use llmsh_policy::engine::{DefaultPolicyConfig, DefaultPolicyEngine};
use llmsh_tools::registry::ToolRegistry;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

fn single_stop_response() -> Vec<LlmResponse> {
    vec![LlmResponse {
        message: Some("ok".into()),
        tool_calls: vec![],
        finish_reason: FinishReason::Stop,
        usage: None,
    }]
}

fn make_deps(
    provider: Arc<MockLlmProvider>,
    agents_md: Option<String>,
    audit_dir: &std::path::Path,
    cwd: &std::path::Path,
) -> Arc<AgentDeps> {
    let registry = Arc::new(ToolRegistry::new());
    let policy = Arc::new(DefaultPolicyEngine::new(DefaultPolicyConfig::default()));
    let pipeline = Pipeline {
        registry: registry.clone(),
        policy,
        home: None,
    };
    let writer = AuditWriter::open(audit_dir, "test-session").unwrap();
    let canonical = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    Arc::new(AgentDeps {
        provider,
        pipeline,
        executor: ToolExecutor {
            registry,
            timeout: Duration::from_secs(5),
            max_output_bytes: 4096,
            env: Default::default(),
            cancel: CancellationToken::new(),
        },
        gate: Arc::new(AlwaysYesGate),
        audit: Mutex::new(writer),
        redactor: Redactor::default_audit(),
        bounds: AgentBounds {
            max_iterations: 5,
            max_tool_calls_per_iteration: 5,
            max_schema_repair_attempts: 2,
        },
        policy_ctx: PolicyContext {
            cwd: canonical.clone(),
            workspace_root: canonical.clone(),
            allowed_roots: vec![canonical],
            sensitive_path_patterns: vec![],
        },
        sensitive_patterns: vec![],
        model_label: "mock:test".into(),
        system_prompt: Arc::new(StaticSystemPrompt { agents_md }),
    })
}

/// LlmRequest.system starts with the persona, no AGENTS.md block when absent.
#[tokio::test]
async fn system_starts_with_persona_no_agents_md() {
    let tmp = tempfile::tempdir().unwrap();
    let audit_dir = tempfile::tempdir().unwrap();

    let provider = Arc::new(MockLlmProvider::new(single_stop_response()));
    let deps = make_deps(provider.clone(), None, audit_dir.path(), tmp.path());

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
    let provider = Arc::new(MockLlmProvider::new(single_stop_response()));
    let deps = make_deps(
        provider.clone(),
        Some(agents_md_content.to_string()),
        audit_dir.path(),
        tmp.path(),
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

    let provider = Arc::new(MockLlmProvider::new(single_stop_response()));
    let deps = make_deps(
        provider.clone(),
        Some("some content".to_string()),
        audit_dir.path(),
        tmp.path(),
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
