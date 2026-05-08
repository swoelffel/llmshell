use serde::{Deserialize, Serialize};
use serde_json::Value;

// v1: initial schema
// v2: added MachineAuditPerformed variant
pub const SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuditEvent {
    SessionStarted {
        ts: String,
        session_id: String,
        cwd: String,
        model: String,
        policy_mode: String,
        llmsh_version: String,
        schema_version: u32,
        config_effective_hash: String,
    },
    UserInput {
        ts: String,
        kind: String,
        text_redacted: String,
    },
    LlmRequest {
        ts: String,
        model: String,
        messages_digest: String,
        tool_count: usize,
        prompt_token_estimate: Option<u32>,
        context_bytes: usize,
        redaction_applied: bool,
        redaction_hit_count: usize,
    },
    LlmResponse {
        ts: String,
        model: String,
        finish_reason: String,
        message_redacted: Option<String>,
        tool_call_count: usize,
        tool_calls_digest: Option<String>,
        usage: Option<Value>,
    },
    ModelPlan {
        ts: String,
        plan_id: String,
        steps_summary: Value,
        steps_digest: String,
    },
    PolicyDecision {
        ts: String,
        plan_id: String,
        step_id: String,
        effective_risk: String,
        action: Value,
        flags: Vec<String>,
        reasons: Vec<String>,
    },
    ConfirmationAsked {
        ts: String,
        plan_id: String,
        phrase: Option<String>,
        granted: bool,
    },
    ToolExecutionStart {
        ts: String,
        plan_id: String,
        step_id: String,
        tool: String,
        args_digest: String,
        args_preview_redacted: Option<Value>,
    },
    ToolExecutionEnd {
        ts: String,
        plan_id: String,
        step_id: String,
        status: String,
        exit_code: Option<i32>,
        stdout_redacted: String,
        stderr_redacted: Option<String>,
        truncated: bool,
        duration_ms: u64,
    },
    RawShellExecution {
        ts: String,
        command_redacted: String,
        status: String,
        exit_code: Option<i32>,
        stdout_redacted: String,
        stderr_redacted: Option<String>,
        truncated: bool,
        risk_scan_hits: Vec<String>,
        duration_ms: u64,
    },
    AssistantMessage {
        ts: String,
        text_redacted: String,
    },
    Error {
        ts: String,
        code: String,
        message: String,
        context_redacted: Option<Value>,
    },
    SessionEnded {
        ts: String,
        reason: String,
    },
    #[serde(rename = "machine_audit_performed")]
    MachineAuditPerformed {
        ts: String,
        host: String,
        os: String,
        user: String,
        tooling_count: usize,
    },
}

pub fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
