mod common;

use async_trait::async_trait;
use llmsh_core::agent::{AgentBounds, AgentDeps, AgentLoop};
use llmsh_core::confirm::AlwaysYesGate;
use llmsh_core::context::{ContextBuilder, StaticSystemPrompt};
use llmsh_core::executor::ToolExecutor;
use llmsh_core::memory::Memory;
use llmsh_llm::types::{FinishReason, LlmResponse, ToolCall};
use llmsh_policy::context::PolicyContext;
use llmsh_policy::engine::{DefaultPolicyConfig, DefaultPolicyEngine};
use llmsh_tools::registry::ToolRegistry;
use llmsh_tools::tool::{Tool, ToolCategory, ToolContext, ToolOutput};
use serde_json::json;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

/// A fake tool that blocks indefinitely until the cancellation token is fired.
struct FakeLongRunningTool;

#[async_trait]
impl Tool for FakeLongRunningTool {
    fn name(&self) -> &str {
        "long_running"
    }
    fn description(&self) -> &str {
        "fake long-running tool"
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({"type": "object", "properties": {}, "additionalProperties": false})
    }
    fn declared_risk(&self) -> llmsh_policy::types::RiskLevel {
        llmsh_policy::types::RiskLevel::ReadOnly
    }
    fn category(&self) -> ToolCategory {
        ToolCategory::System
    }
    async fn execute(
        &self,
        _args: serde_json::Value,
        ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        ctx.cancel.cancelled().await;
        anyhow::bail!("cancelled")
    }
}

/// The cancel token is fired externally after 100 ms.  The agent must audit a
/// `tool_execution_end` with `status = "cancelled"`.
#[tokio::test]
async fn external_cancel_recorded_in_audit() {
    let tmp = tempfile::tempdir().unwrap();

    let scripted = vec![LlmResponse {
        message: None,
        tool_calls: vec![ToolCall {
            id: "c1".into(),
            name: "long_running".into(),
            args: json!({}),
        }],
        finish_reason: FinishReason::ToolCalls,
        usage: None,
    }];

    let mut reg = ToolRegistry::new();
    reg.register(Arc::new(FakeLongRunningTool));
    let registry = Arc::new(reg);

    let cancel = CancellationToken::new();
    let audit_dir = tempfile::tempdir().unwrap();
    let writer = llmsh_audit::writer::AuditWriter::open(audit_dir.path(), "test-session").unwrap();
    let policy = Arc::new(DefaultPolicyEngine::new(DefaultPolicyConfig::default()));
    let pipeline = llmsh_core::pipeline::Pipeline {
        registry: registry.clone(),
        policy,
        home: None,
    };

    let deps = Arc::new(AgentDeps {
        provider: Arc::new(common::MockLlmProvider::new(scripted)),
        pipeline,
        executor: ToolExecutor {
            registry,
            timeout: Duration::from_secs(30),
            max_output_bytes: 4096,
            env: Default::default(),
            cancel: cancel.clone(),
        },
        gate: Arc::new(AlwaysYesGate),
        audit: Mutex::new(writer),
        redactor: llmsh_audit::redact::Redactor::default_audit(),
        bounds: AgentBounds {
            max_iterations: 5,
            max_tool_calls_per_iteration: 5,
            max_schema_repair_attempts: 2,
        },
        policy_ctx: PolicyContext {
            cwd: tmp.path().to_path_buf(),
            workspace_root: tmp.path().to_path_buf(),
            allowed_roots: vec![tmp.path().to_path_buf()],
            sensitive_path_patterns: vec![],
        },
        sensitive_patterns: vec![],
        model_label: Arc::new(RwLock::new("mock:test".into())),
        system_prompt: Arc::new(StaticSystemPrompt::new(None)),
        memory: Arc::new(Memory::open_in_memory().unwrap()),
        verbose: 0,
        stats: Arc::new(std::sync::RwLock::new(
            llmsh_core::session_stats::SessionStats::default(),
        )),
    });

    // Fire the cancel token after 100 ms while the agent loop is running.
    let cancel_handle = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancel_handle.cancel();
    });

    let mut agent = AgentLoop {
        deps: deps.clone(),
        builder: ContextBuilder::new(4096),
    };
    // The agent will either error (no more scripted responses after tool result)
    // or loop; either way the tool_execution_end should be recorded.
    let _ = agent.run("run long task").await;

    deps.audit.lock().unwrap().flush().unwrap();
    let log = std::fs::read_to_string(audit_dir.path().join("test-session.jsonl")).unwrap();

    assert!(
        log.contains("\"type\":\"tool_execution_end\""),
        "expected tool_execution_end in audit log"
    );
    assert!(
        log.contains("\"status\":\"cancelled\""),
        "expected status=cancelled in tool_execution_end"
    );
}
