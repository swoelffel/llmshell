use serde::{Deserialize, Serialize};
use serde_json::Value;

// v1: initial schema
// v2: added MachineAuditPerformed variant
// v3: added ContextCompacted variant
// v4: added ContextCleared and FactAdded variants
pub const SCHEMA_VERSION: u32 = 4;

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
    MachineAuditPerformed {
        ts: String,
        host: String,
        os: String,
        user: String,
        tooling_count: usize,
    },
    ModelChanged {
        ts: String,
        from: String,
        to: String,
    },
    ContextCompacted {
        ts: String,
        reason: String,
        strategy: String,
        messages_before: usize,
        messages_after: usize,
        bytes_before: usize,
        bytes_after: usize,
        summary_digest: Option<String>,
    },
    ContextCleared {
        ts: String,
        scope: String, // 'context'|'memory'|'all'|'memory_forget'
        rows_affected: usize,
    },
    FactAdded {
        ts: String,
        fact_id: i64,
        category: String,
        source: String, // 'manual'|'compact'|'init'
    },
}

pub fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_audit_performed_serializes_as_snake_case() {
        let ev = AuditEvent::MachineAuditPerformed {
            ts: "2026-05-08T10:42:00Z".into(),
            host: "h".into(),
            os: "macOS 25.3.0 (Darwin arm64)".into(),
            user: "u".into(),
            tooling_count: 7,
        };
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["type"], "machine_audit_performed");
        assert_eq!(json["tooling_count"], 7);
    }

    #[test]
    fn context_cleared_roundtrip() {
        let ev = AuditEvent::ContextCleared {
            ts: "2026-05-09T00:00:00.000Z".into(),
            scope: "context".into(),
            rows_affected: 5,
        };
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["type"], "context_cleared");
        assert_eq!(json["scope"], "context");
        assert_eq!(json["rows_affected"], 5);
        let roundtrip: AuditEvent = serde_json::from_value(json).unwrap();
        if let AuditEvent::ContextCleared {
            scope,
            rows_affected,
            ..
        } = roundtrip
        {
            assert_eq!(scope, "context");
            assert_eq!(rows_affected, 5);
        } else {
            panic!("wrong variant");
        }
    }

    #[test]
    fn fact_added_roundtrip() {
        let ev = AuditEvent::FactAdded {
            ts: "2026-05-09T00:00:00.000Z".into(),
            fact_id: 42,
            category: "preference".into(),
            source: "manual".into(),
        };
        let json = serde_json::to_value(&ev).unwrap();
        assert_eq!(json["type"], "fact_added");
        assert_eq!(json["fact_id"], 42);
        assert_eq!(json["category"], "preference");
        assert_eq!(json["source"], "manual");
        let roundtrip: AuditEvent = serde_json::from_value(json).unwrap();
        if let AuditEvent::FactAdded {
            fact_id,
            category,
            source,
            ..
        } = roundtrip
        {
            assert_eq!(fact_id, 42);
            assert_eq!(category, "preference");
            assert_eq!(source, "manual");
        } else {
            panic!("wrong variant");
        }
    }
}
