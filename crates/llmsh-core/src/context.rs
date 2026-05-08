use crate::executor::StepResult;
use crate::llm_redact::LlmRedactor;
use crate::memory::{ActionKind, Memory};
use llmsh_llm::types::{Message, MessageRole};
use std::sync::Arc;

pub const DEFAULT_PERSONA: &str = "\
You are LLMShell, an agentic shell assistant installed on this machine. \
Help the user accomplish tasks via natural language, calling typed runtime tools \
(prefer them over run_process). \
Never assume actions succeeded until tool results confirm them. \
The Runtime context and Recent activity sections below describe the current machine \
state and the latest actions in this session — use them to ground your answers.";

// The four Option<String> slots below are filled by later phases (B/C/D/E);
// Phase A leaves them None.
pub struct SystemPromptBuilder {
    persona: &'static str,
    pub agents_md: Option<String>,
    pub long_term_memory: Option<String>,
    pub runtime_context: Option<String>,
    pub recent_activity: Option<String>,
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
        fn non_empty(s: &Option<String>) -> Option<&str> {
            s.as_deref().filter(|v| !v.is_empty())
        }

        let mut out = self.persona.to_string();
        let push = |out: &mut String, header: &str, body: &str| {
            out.push_str("\n\n");
            out.push_str(header);
            out.push('\n');
            out.push_str(body);
        };

        if let Some(body) = non_empty(&self.agents_md) {
            push(&mut out, "=== AGENTS.md ===", body);
        }
        if let Some(body) = non_empty(&self.long_term_memory) {
            push(&mut out, "=== Long-term memory ===", body);
        }
        if let Some(body) = non_empty(&self.runtime_context) {
            push(&mut out, "=== Runtime context ===", body);
        }
        if let Some(body) = non_empty(&self.recent_activity) {
            push(&mut out, "=== Recent activity ===", body);
        }
        out
    }
}

impl Default for SystemPromptBuilder {
    fn default() -> Self {
        Self::new()
    }
}

// Phase A constructs the system prompt from a struct — later phases replace it
// with a richer struct that also injects runtime_context and recent_activity.
// Using a trait object keeps AgentDeps extensible without threading concrete
// types through every callsite that only needs `current()`.
pub trait SystemPromptSource: Send + Sync {
    fn current(&self) -> String;
}

pub struct StaticSystemPrompt {
    agents_md: Option<String>,
}

impl StaticSystemPrompt {
    pub fn new(agents_md: Option<String>) -> Self {
        Self { agents_md }
    }
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

pub struct MemorySystemPrompt {
    agents_md: Option<String>,
    memory: Arc<Memory>,
}

impl MemorySystemPrompt {
    pub fn new(agents_md: Option<String>, memory: Arc<Memory>) -> Self {
        Self { agents_md, memory }
    }

    fn format_recent_activity(&self) -> Option<String> {
        let actions = self.memory.last_actions(3).ok()?;
        if actions.is_empty() {
            return None;
        }
        let lines: Vec<String> = actions
            .iter()
            .map(|a| match a.kind {
                ActionKind::UserInput => format!("user: {}", a.summary),
                ActionKind::Assistant => format!("assistant: {}", a.summary),
                ActionKind::Tool => match a.summary.split_once(": ") {
                    Some((name, rest)) => format!("tool[{}]: {}", name, rest),
                    None => format!("tool[?]: {}", a.summary),
                },
            })
            .collect();
        Some(lines.join("\n"))
    }
}

impl SystemPromptSource for MemorySystemPrompt {
    fn current(&self) -> String {
        let mut b = SystemPromptBuilder::new();
        b.agents_md = self.agents_md.clone();
        b.long_term_memory = self
            .memory
            .read_init_audit()
            .ok()
            .flatten()
            .map(|a| a.summary_md);
        b.recent_activity = self.format_recent_activity();
        b.build()
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

    #[test]
    fn memory_system_prompt_tool_line_format() {
        use crate::memory::{ActionKind, Memory, RecentAction};

        let memory = Arc::new(Memory::open_in_memory().unwrap());
        memory
            .append_action(&RecentAction {
                ts: "2026-01-01T00:00:00.000Z".into(),
                kind: ActionKind::Tool,
                summary: "list_directory: success".into(),
                detail_json: None,
            })
            .unwrap();

        let prompt = MemorySystemPrompt::new(None, memory);
        let activity = prompt.format_recent_activity().unwrap();
        assert_eq!(activity, "tool[list_directory]: success");
    }

    #[test]
    fn memory_system_prompt_mixed_kinds_formatting() {
        use crate::memory::{ActionKind, Memory, RecentAction};

        let memory = Arc::new(Memory::open_in_memory().unwrap());
        for (kind, summary) in [
            (ActionKind::UserInput, "hello"),
            (ActionKind::Assistant, "hi there"),
            (ActionKind::Tool, "read_file: failed"),
        ] {
            memory
                .append_action(&RecentAction {
                    ts: "2026-01-01T00:00:00.000Z".into(),
                    kind,
                    summary: summary.into(),
                    detail_json: None,
                })
                .unwrap();
        }

        let prompt = MemorySystemPrompt::new(None, memory);
        let activity = prompt.format_recent_activity().unwrap();
        let lines: Vec<&str> = activity.lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "user: hello");
        assert_eq!(lines[1], "assistant: hi there");
        assert_eq!(lines[2], "tool[read_file]: failed");
    }
}
