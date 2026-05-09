use crate::executor::StepResult;
use crate::llm_redact::LlmRedactor;
use crate::memory::Memory;
use crate::sysctx::RuntimeContext;
use llmsh_llm::types::{Message, MessageRole, ToolCall};
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Instant;

pub const DEFAULT_PERSONA: &str = "\
You are LLMShell, an agentic shell assistant installed on this machine. \
Help the user accomplish tasks via natural language, calling typed runtime tools \
(prefer them over run_process). \
Never assume actions succeeded until tool results confirm them. \
The Runtime context section below describes the current machine state — use it to ground your answers.";

pub struct SystemPromptBuilder {
    persona: &'static str,
    pub agents_md: Option<String>,
    pub long_term_memory: Option<String>, // existing: init_audit summary
    pub long_term_facts: Option<String>,  // NEW: curated facts
    pub runtime_context: Option<String>,
}

impl SystemPromptBuilder {
    pub fn new() -> Self {
        Self {
            persona: DEFAULT_PERSONA,
            agents_md: None,
            long_term_memory: None,
            long_term_facts: None,
            runtime_context: None,
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
        if let Some(body) = non_empty(&self.long_term_facts) {
            push(&mut out, "=== Long-term facts ===", body);
        }
        if let Some(body) = non_empty(&self.runtime_context) {
            push(&mut out, "=== Runtime context ===", body);
        }
        out
    }
}

impl Default for SystemPromptBuilder {
    fn default() -> Self {
        Self::new()
    }
}

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
    workspace_root: PathBuf,
    model: Arc<RwLock<String>>,
    session_start: Instant,
}

impl MemorySystemPrompt {
    pub fn new(
        agents_md: Option<String>,
        memory: Arc<Memory>,
        workspace_root: PathBuf,
        model: Arc<RwLock<String>>,
        session_start: Instant,
    ) -> Self {
        let workspace_root = std::fs::canonicalize(&workspace_root).unwrap_or(workspace_root);
        Self {
            agents_md,
            memory,
            workspace_root,
            model,
            session_start,
        }
    }

    fn format_long_term_facts(&self) -> Option<String> {
        let facts = self.memory.load_active_facts().ok()?;
        if facts.is_empty() {
            return None;
        }
        let lines: Vec<String> = facts
            .iter()
            .map(|f| format!("- [{}] {}", f.category, f.claim))
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
        let rt = RuntimeContext::capture(
            self.workspace_root.clone(),
            self.model.clone(),
            self.session_start,
        );
        b.long_term_facts = self.format_long_term_facts();
        b.runtime_context = Some(rt.render());
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
            tool_calls: None,
        });
    }

    pub fn append_assistant(&mut self, text: &str) {
        let (red, _) = self.redactor.redact(text);
        self.messages.push(Message {
            role: MessageRole::Assistant,
            content: red,
            tool_call_id: None,
            name: None,
            tool_calls: None,
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
                tool_calls: None,
            });
        }
    }

    pub fn append_assistant_with_tool_calls(
        &mut self,
        text: Option<&str>,
        tool_calls: Vec<ToolCall>,
    ) {
        let content = text.map(|t| self.redactor.redact(t).0).unwrap_or_default();
        self.messages.push(Message {
            role: MessageRole::Assistant,
            content,
            tool_call_id: None,
            name: None,
            tool_calls: if tool_calls.is_empty() {
                None
            } else {
                Some(tool_calls)
            },
        });
    }

    pub fn append_user_cancellation(&mut self) {
        self.messages.push(Message {
            role: MessageRole::User,
            content: "User cancelled the proposed action.".into(),
            tool_call_id: None,
            name: None,
            tool_calls: None,
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
            tool_calls: None,
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
    fn builder_persists_messages_across_runs() {
        // The ContextBuilder must accumulate messages when reused across
        // multiple agent turns — that's how the LLM sees the conversation.
        let mut b = ContextBuilder::new(4096);
        b.append_user("first turn");
        b.append_assistant("first reply");
        b.append_user("second turn");
        b.append_assistant("second reply");

        assert_eq!(b.messages.len(), 4);
        assert_eq!(b.messages[0].role, MessageRole::User);
        assert_eq!(b.messages[0].content, "first turn");
        assert_eq!(b.messages[1].role, MessageRole::Assistant);
        assert_eq!(b.messages[1].content, "first reply");
        assert_eq!(b.messages[2].role, MessageRole::User);
        assert_eq!(b.messages[2].content, "second turn");
        assert_eq!(b.messages[3].role, MessageRole::Assistant);
        assert_eq!(b.messages[3].content, "second reply");
    }

    #[test]
    fn clearing_messages_resets_to_empty_but_preserves_settings() {
        let mut b = ContextBuilder::new(8192);
        let original_max = b.max_llm_output_bytes;
        b.append_user("hello");
        b.append_assistant("hi");
        assert_eq!(b.messages.len(), 2);

        b.messages.clear();
        assert!(b.messages.is_empty());
        assert_eq!(b.max_llm_output_bytes, original_max);
    }

    #[test]
    fn append_assistant_with_tool_calls_then_tool_results_orders_correctly() {
        use crate::executor::{ExecutionStatus, StepResult};
        use llmsh_llm::types::ToolCall;
        use llmsh_tools::tool::ToolOutput;
        use serde_json::json;
        use std::time::Duration;

        let mut b = ContextBuilder::new(4096);
        b.append_user("list");
        b.append_assistant_with_tool_calls(
            None,
            vec![ToolCall {
                id: "call_x".into(),
                name: "list_directory".into(),
                args: json!({"path": "."}),
            }],
        );
        b.append_tool_results(&[StepResult {
            step_id: "call_x".into(),
            tool_name: "list_directory".into(),
            status: ExecutionStatus::Success,
            output: Some(ToolOutput {
                stdout: "ok".into(),
                stderr: None,
                exit_code: Some(0),
                truncated: false,
                structured: None,
            }),
            error: None,
            duration: Duration::from_millis(1),
        }]);

        assert_eq!(b.messages.len(), 3);
        assert_eq!(b.messages[0].role, MessageRole::User);
        assert_eq!(b.messages[1].role, MessageRole::Assistant);
        let tcs = b.messages[1].tool_calls.as_ref().unwrap();
        assert_eq!(tcs.len(), 1);
        assert_eq!(tcs[0].id, "call_x");
        assert_eq!(b.messages[2].role, MessageRole::Tool);
        assert_eq!(b.messages[2].tool_call_id.as_deref(), Some("call_x"));
    }

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
    fn all_sections_correct_order() {
        let b = SystemPromptBuilder {
            agents_md: Some("agents".into()),
            long_term_memory: Some("memory".into()),
            long_term_facts: Some("facts".into()),
            runtime_context: Some("runtime".into()),
            ..SystemPromptBuilder::new()
        };
        let out = b.build();
        let agents_pos = out.find("=== AGENTS.md ===").unwrap();
        let memory_pos = out.find("=== Long-term memory ===").unwrap();
        let facts_pos = out.find("=== Long-term facts ===").unwrap();
        let runtime_pos = out.find("=== Runtime context ===").unwrap();
        assert!(agents_pos < memory_pos);
        assert!(memory_pos < facts_pos);
        assert!(facts_pos < runtime_pos);
        // Persona is first — no header before it
        assert!(out.starts_with(DEFAULT_PERSONA));
        // No legacy section
        assert!(!out.contains("=== Recent activity ==="));
    }

    #[test]
    fn empty_string_sections_omitted() {
        let b = SystemPromptBuilder {
            agents_md: Some("".into()),
            long_term_memory: Some("".into()),
            long_term_facts: Some("".into()),
            runtime_context: Some("".into()),
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

    fn make_test_prompt(memory: Arc<Memory>) -> MemorySystemPrompt {
        MemorySystemPrompt::new(
            None,
            memory,
            std::path::PathBuf::from("/tmp"),
            Arc::new(RwLock::new("mock:test".to_string())),
            std::time::Instant::now(),
        )
    }

    #[test]
    fn memory_system_prompt_facts_section_renders() {
        use crate::memory::Memory;
        let memory = Arc::new(Memory::open_in_memory().unwrap());
        memory
            .replace_facts_generation(
                "2026-01-01T00:00:00.000Z",
                &[
                    ("identity".into(), "user is Stéphane".into()),
                    ("preference".into(), "prefers French replies".into()),
                ],
            )
            .unwrap();
        let prompt = make_test_prompt(memory);
        let body = prompt.format_long_term_facts().unwrap();
        assert!(body.contains("[identity] user is Stéphane"));
        assert!(body.contains("[preference] prefers French replies"));
    }

    #[test]
    fn memory_system_prompt_no_facts_returns_none() {
        let memory = Arc::new(Memory::open_in_memory().unwrap());
        let prompt = make_test_prompt(memory);
        assert!(prompt.format_long_term_facts().is_none());
    }

    #[test]
    fn system_prompt_includes_facts_section_when_present() {
        use crate::memory::Memory;
        let memory = Arc::new(Memory::open_in_memory().unwrap());
        memory
            .replace_facts_generation(
                "2026-01-01T00:00:00.000Z",
                &[("identity".into(), "claim".into())],
            )
            .unwrap();
        let prompt = make_test_prompt(memory);
        let out = prompt.current();
        assert!(out.contains("=== Long-term facts ==="));
        assert!(out.contains("[identity] claim"));
        assert!(!out.contains("=== Recent activity ==="));
    }
}
