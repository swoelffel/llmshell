//! Centralised secret redaction for LLMShell.
//!
//! Single source of truth — replaces the three previous parallel implementations
//! in `llmsh-audit::redact`, `llmsh-core::llm_redact`, and `llmsh-core::raw_shell`.

mod engine;
mod patterns;

pub use engine::Redactor;
pub use patterns::{default_patterns, PatternDef};
