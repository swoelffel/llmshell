use crate::executor::StepResult;
use crate::llm_redact::LlmRedactor;
use llmsh_llm::types::{Message, MessageRole};

pub const SYSTEM_PROMPT: &str = "\
You are LLMShell, an agentic shell assistant. The user expresses intent in natural language. \
You can call tools provided by the runtime. \
Prefer typed tools over run_process. Only use run_process as a last resort, \
when no typed tool covers the requested action. \
Never assume actions have been performed until tool results confirm them.";

pub struct ContextBuilder {
    pub messages: Vec<Message>,
    pub redactor: LlmRedactor,
    pub max_llm_output_bytes: usize,
}

impl ContextBuilder {
    pub fn new(max_llm_output_bytes: usize) -> Self {
        Self {
            messages: vec![],
            redactor: LlmRedactor::default(),
            max_llm_output_bytes,
        }
    }

    pub fn append_user(&mut self, text: &str) {
        let (red, _) = self.redactor.redact(text);
        self.messages.push(Message {
            role: MessageRole::User,
            content: red,
            tool_call_id: None,
            name: None,
        });
    }

    pub fn append_assistant(&mut self, text: &str) {
        let (red, _) = self.redactor.redact(text);
        self.messages.push(Message {
            role: MessageRole::Assistant,
            content: red,
            tool_call_id: None,
            name: None,
        });
    }

    pub fn append_tool_results(&mut self, results: &[StepResult]) {
        for r in results {
            let raw = match (&r.output, &r.error) {
                (Some(o), _) => format!(
                    "{{\"status\":\"{}\",\"stdout\":{},\"exit_code\":{},\"truncated\":{}}}",
                    status_str(&r.status),
                    serde_json::to_string(&o.stdout).unwrap_or_else(|_| "\"\"".into()),
                    o.exit_code
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "null".into()),
                    o.truncated,
                ),
                (None, Some(e)) => format!(
                    "{{\"status\":\"{}\",\"error\":{}}}",
                    status_str(&r.status),
                    serde_json::to_string(e).unwrap_or_else(|_| "\"\"".into()),
                ),
                _ => "{}".to_string(),
            };
            let (red, _) = self.redactor.redact(&raw);
            let (clipped, _) = self.redactor.truncate(&red, self.max_llm_output_bytes);
            self.messages.push(Message {
                role: MessageRole::Tool,
                content: clipped,
                tool_call_id: Some(r.step_id.clone()),
                name: Some(r.tool_name.clone()),
            });
        }
    }

    pub fn append_user_cancellation(&mut self) {
        self.messages.push(Message {
            role: MessageRole::User,
            content: "User cancelled the proposed action.".into(),
            tool_call_id: None,
            name: None,
        });
    }

    pub fn append_schema_error(&mut self, tool_call_id: &str, message: &str) {
        let body = serde_json::json!({
            "status": "error", "error_type": "schema_validation", "message": message
        });
        self.messages.push(Message {
            role: MessageRole::Tool,
            content: body.to_string(),
            tool_call_id: Some(tool_call_id.into()),
            name: None,
        });
    }
}

fn status_str(s: &crate::executor::ExecutionStatus) -> &'static str {
    match s {
        crate::executor::ExecutionStatus::Success => "success",
        crate::executor::ExecutionStatus::Failed => "failed",
        crate::executor::ExecutionStatus::Cancelled => "cancelled",
        crate::executor::ExecutionStatus::TimedOut => "timed_out",
    }
}
