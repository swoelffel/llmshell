pub mod summarize;
pub mod truncate;
pub mod validate;

use llmsh_llm::types::Message;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionReason {
    Manual,
    Auto,
}

impl CompactionReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Auto => "auto",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionStrategy {
    Truncate,
    Summarize,
    Both,
    Noop,
}

impl CompactionStrategy {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Truncate => "truncate",
            Self::Summarize => "summarize",
            Self::Both => "both",
            Self::Noop => "noop",
        }
    }
}

#[derive(Debug, Clone)]
pub struct CompactionReport {
    pub reason: CompactionReason,
    pub strategy: CompactionStrategy,
    pub messages_before: usize,
    pub messages_after: usize,
    pub bytes_before: usize,
    pub bytes_after: usize,
    pub summary_digest: Option<String>,
}

/// Total bytes occupied by the `content` of every message in `messages`.
pub fn total_content_bytes(messages: &[Message]) -> usize {
    messages.iter().map(|m| m.content.len()).sum()
}
