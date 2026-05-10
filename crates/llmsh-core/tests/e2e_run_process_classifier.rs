//! E2E: deterministic classifier downgrades safe `run_process` invocations,
//! and the opt-out flag restores the legacy behaviour.

mod common;

use common::MockLlmProvider;
use llmsh_audit::redact::Redactor;
use llmsh_audit::writer::AuditWriter;
use llmsh_core::agent::{AgentBounds, AgentDeps, AgentLoop};
use llmsh_core::config::{CompactConfig, MemoryConfig};
use llmsh_core::confirm::{AlwaysNoGate, ConfirmationGate};
use llmsh_core::context::{ContextBuilder, StaticSystemPrompt};
use llmsh_core::executor::ToolExecutor;
use llmsh_core::memory::Memory;
use llmsh_core::pipeline::Pipeline;
use llmsh_llm::types::{FinishReason, LlmResponse, ToolCall};
use llmsh_policy::context::PolicyContext;
use llmsh_policy::engine::{DefaultPolicyConfig, DefaultPolicyEngine};
use llmsh_tools::registry::ToolRegistry;
use llmsh_tools::run_process::RunProcess;
use serde_json::json;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

fn build_deps(
    scripted: Vec<LlmResponse>,
    gate: Arc<dyn ConfirmationGate>,
    auto_classify: bool,
    audit_dir: &std::path::Path,
) -> Arc<AgentDeps> {
    let mut reg = ToolRegistry::new();
    reg.register(Arc::new(RunProcess));
    let registry = Arc::new(reg);

    let writer = AuditWriter::open(audit_dir, "test-session").unwrap();
    let policy = Arc::new(DefaultPolicyEngine::new(DefaultPolicyConfig::default()));
    let pipeline = Pipeline {
        registry: registry.clone(),
        policy,
        home: None,
        auto_classify_run_process: auto_classify,
    };

    let cwd = std::env::temp_dir();
    let canonical = std::fs::canonicalize(&cwd).unwrap_or(cwd);

    Arc::new(AgentDeps {
        provider: Arc::new(MockLlmProvider::new(scripted)),
        pipeline,
        executor: ToolExecutor {
            registry,
            timeout: Duration::from_secs(5),
            max_output_bytes: 4096,
            env: Default::default(),
            cancel: CancellationToken::new(),
            home: None,
        },
        gate,
        audit: Mutex::new(writer),
        redactor: Redactor::default_audit(),
        bounds: AgentBounds {
            max_iterations: 3,
            max_tool_calls_per_iteration: 5,
            max_schema_repair_attempts: 2,
        },
        compact_config: CompactConfig::default(),
        memory_cfg: MemoryConfig::default(),
        policy_ctx: PolicyContext {
            cwd: Arc::new(RwLock::new(canonical.clone())),
            workspace_root: canonical.clone(),
            allowed_roots: vec![canonical],
            sensitive_path_patterns: vec![],
        },
        sensitive_patterns: vec![],
        model_label: Arc::new(RwLock::new("mock:test".into())),
        system_prompt: Arc::new(StaticSystemPrompt::new(None)),
        memory: Arc::new(Memory::open_in_memory().unwrap()),
        verbose: 0,
        stats: Arc::new(RwLock::new(
            llmsh_core::session_stats::SessionStats::default(),
        )),
        oldpwd: Arc::new(Mutex::new(None)),
        home: None,
    })
}

#[tokio::test]
async fn auto_classify_skips_confirmation_for_ls() {
    let scripted = vec![
        LlmResponse {
            message: None,
            tool_calls: vec![ToolCall {
                id: "c1".into(),
                name: "run_process".into(),
                args: json!({
                    "program":"ls","args":["-la","/tmp"],
                    "intent":"list /tmp","claimed_risk":"read_only"
                }),
            }],
            finish_reason: FinishReason::ToolCalls,
            usage: None,
        },
        LlmResponse {
            message: Some("done".into()),
            tool_calls: vec![],
            finish_reason: FinishReason::Stop,
            usage: None,
        },
    ];

    let audit_dir = tempfile::tempdir().unwrap();
    // AlwaysNoGate would deny if the gate were reached. The fact that the
    // turn ends with `stop` proves the gate was NOT reached for this call.
    let deps = build_deps(scripted, Arc::new(AlwaysNoGate), true, audit_dir.path());

    let mut agent = AgentLoop {
        deps: deps.clone(),
        builder: ContextBuilder::new(4096),
    };
    let res = agent.run("list /tmp").await.unwrap();
    assert_eq!(res.stopped_reason, "stop");

    deps.audit.lock().unwrap().flush().unwrap();
    let log = std::fs::read_to_string(audit_dir.path().join("test-session.jsonl")).unwrap();
    assert!(log.contains("\"effective_risk\":\"readonly\""));
    assert!(log.contains("\"KnownReadOnlyCommand\""));
    // No confirmation should have been asked for an auto-classified call.
    assert!(!log.contains("\"type\":\"confirmation_asked\""));
}

#[tokio::test]
async fn auto_classify_disabled_falls_back_to_confirmation() {
    let scripted = vec![LlmResponse {
        message: None,
        tool_calls: vec![ToolCall {
            id: "c1".into(),
            name: "run_process".into(),
            args: json!({
                "program":"ls",
                "intent":"list","claimed_risk":"read_only"
            }),
        }],
        finish_reason: FinishReason::ToolCalls,
        usage: None,
    }];

    let audit_dir = tempfile::tempdir().unwrap();
    let deps = build_deps(scripted, Arc::new(AlwaysNoGate), false, audit_dir.path());

    let mut agent = AgentLoop {
        deps: deps.clone(),
        builder: ContextBuilder::new(4096),
    };
    let res = agent.run("list").await.unwrap();
    assert_eq!(res.stopped_reason, "cancelled");
}

#[tokio::test]
async fn rm_still_requires_confirmation() {
    let scripted = vec![LlmResponse {
        message: None,
        tool_calls: vec![ToolCall {
            id: "c1".into(),
            name: "run_process".into(),
            args: json!({
                "program":"rm","args":["-rf","/tmp/x"],
                "intent":"clean","claimed_risk":"destructive"
            }),
        }],
        finish_reason: FinishReason::ToolCalls,
        usage: None,
    }];

    let audit_dir = tempfile::tempdir().unwrap();
    let deps = build_deps(scripted, Arc::new(AlwaysNoGate), true, audit_dir.path());

    let mut agent = AgentLoop {
        deps: deps.clone(),
        builder: ContextBuilder::new(4096),
    };
    let res = agent.run("delete").await.unwrap();
    assert_eq!(res.stopped_reason, "cancelled");
}
