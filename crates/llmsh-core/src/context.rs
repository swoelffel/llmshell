use crate::executor::StepResult;
use crate::llm_redact::LlmRedactor;
use llmsh_llm::types::{Message, MessageRole};

pub const DEFAULT_PERSONA: &str = "\
You are LLMShell, an agentic shell assistant installed on this machine. \
Help the user accomplish tasks via natural language, calling typed runtime tools \
(prefer them over run_process). \
Never assume actions succeeded until tool results confirm them. \
The Runtime context and Recent activity sections below describe the current machine \
state and the latest actions in this session — use them to ground your answers.";

pub struct SystemPromptBuilder {
    pub persona: &'static str,
    pub agents_md: Option<String>,
    pub long_term_memory: Option<String>, // Phase B/D
    pub runtime_context: Option<String>,  // Phase C
    pub recent_activity: Option<String>,  // Phase B
}

impl SystemPromptBuilder {
    pub fn new() -> Self {
        Self {
            persona: DEFAULT_PERSONA,
            agents_md: None,
            long_term_memory: None,
            runtime_context: None,
            recent_activity: None,
        }
    }

    pub fn build(&self) -> String {
        let mut parts: Vec<&str> = vec![self.persona];

        // Helper to check whether an optional section has non-empty content.
        fn non_empty(s: &Option<String>) -> Option<&str> {
            s.as_deref().filter(|v| !v.is_empty())
        }

        // Sections are stored as owned strings so we build headers inline.
        // We collect (header, body) pairs to avoid lifetime friction.
        let mut sections: Vec<String> = vec![];

        if let Some(body) = non_empty(&self.agents_md) {
            sections.push(format!("=== AGENTS.md ===\n{body}"));
        }
        if let Some(body) = non_empty(&self.long_term_memory) {
            sections.push(format!("=== Long-term memory ===\n{body}"));
        }
        if let Some(body) = non_empty(&self.runtime_context) {
            sections.push(format!("=== Runtime context ===\n{body}"));
        }
        if let Some(body) = non_empty(&self.recent_activity) {
            sections.push(format!("=== Recent activity ===\n{body}"));
        }

        // Build the final prompt: persona first (stable prefix), then sections.
        let mut out = parts.remove(0).to_string();
        for section in &sections {
            out.push_str("\n\n");
            out.push_str(section);
        }
        out
    }
}

impl Default for SystemPromptBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// Phase A constructs the system prompt from a closure — later phases replace it
// with a richer closure that also injects runtime_context and recent_activity.
// Using a trait object keeps AgentDeps extensible without threading concrete
// types through every callsite that only needs `current()`.
pub trait SystemPromptSource: Send + Sync {
    fn current(&self) -> String;
}

pub struct StaticSystemPrompt {
    pub agents_md: Option<String>,
}

impl SystemPromptSource for StaticSystemPrompt {
    fn current(&self) -> String {
        SystemPromptBuilder {
            agents_md: self.agents_md.clone(),
            ..SystemPromptBuilder::new()
        }
        .build()
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_persona_no_headers() {
        let b = SystemPromptBuilder::new();
        let out = b.build();
        assert_eq!(out, DEFAULT_PERSONA);
        assert!(!out.contains("==="));
    }

    #[test]
    fn persona_plus_agents_md_two_sections() {
        let b = SystemPromptBuilder {
            agents_md: Some("Be concise.".into()),
            ..SystemPromptBuilder::new()
        };
        let out = b.build();
        let expected = format!("{}\n\n=== AGENTS.md ===\nBe concise.", DEFAULT_PERSONA);
        assert_eq!(out, expected);
    }

    #[test]
    fn all_five_sections_correct_order() {
        let b = SystemPromptBuilder {
            agents_md: Some("agents".into()),
            long_term_memory: Some("memory".into()),
            runtime_context: Some("runtime".into()),
            recent_activity: Some("activity".into()),
            ..SystemPromptBuilder::new()
        };
        let out = b.build();
        let agents_pos = out.find("=== AGENTS.md ===").unwrap();
        let memory_pos = out.find("=== Long-term memory ===").unwrap();
        let runtime_pos = out.find("=== Runtime context ===").unwrap();
        let activity_pos = out.find("=== Recent activity ===").unwrap();
        assert!(agents_pos < memory_pos);
        assert!(memory_pos < runtime_pos);
        assert!(runtime_pos < activity_pos);
        // Persona is first — no header before it
        assert!(out.starts_with(DEFAULT_PERSONA));
    }

    #[test]
    fn empty_string_sections_omitted() {
        let b = SystemPromptBuilder {
            agents_md: Some("".into()),
            long_term_memory: Some("".into()),
            runtime_context: Some("".into()),
            recent_activity: Some("".into()),
            ..SystemPromptBuilder::new()
        };
        let out = b.build();
        assert_eq!(out, DEFAULT_PERSONA);
        assert!(!out.contains("==="));
    }

    #[test]
    fn output_starts_with_persona() {
        let b = SystemPromptBuilder {
            agents_md: Some("something".into()),
            runtime_context: Some("os: linux".into()),
            ..SystemPromptBuilder::new()
        };
        let out = b.build();
        assert!(out.starts_with(DEFAULT_PERSONA));
    }

    #[test]
    fn deterministic_output_same_inputs() {
        let build = || {
            SystemPromptBuilder {
                agents_md: Some("hello".into()),
                ..SystemPromptBuilder::new()
            }
            .build()
        };
        assert_eq!(build(), build());
    }
}
