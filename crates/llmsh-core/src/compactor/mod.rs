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

use crate::config::CompactConfig;
use llmsh_llm::context_window::context_window_for;
use llmsh_llm::provider::LlmProvider;
use std::sync::Arc;

/// Run the cascade (stage A then optionally B) on `messages` in place.
///
/// `last_input_tokens` is the prompt_tokens of the most recent provider
/// response — used to decide whether to trigger stage B for the auto path.
/// For the manual path, pass `u32::MAX` to force stage B.
pub async fn compact(
    messages: &mut Vec<llmsh_llm::types::Message>,
    cfg: &CompactConfig,
    reason: CompactionReason,
    model: &str,
    last_input_tokens: u32,
    provider: Arc<dyn LlmProvider>,
) -> CompactionReport {
    let messages_before = messages.len();
    let bytes_before = total_content_bytes(messages);

    // Stage A — deterministic truncation of tool outputs.
    let truncated = truncate::truncate_tool_outputs(messages, cfg.tool_output_max_bytes);

    // Decide if stage B should run.
    let should_summarize = match reason {
        CompactionReason::Manual => true,
        CompactionReason::Auto => {
            let window = context_window_for(model);
            let threshold = (window as u64 * cfg.auto_threshold_pct as u64 / 100) as u32;
            cfg.auto_threshold_pct > 0 && last_input_tokens >= threshold
        }
    };

    let mut summarized = false;
    let mut digest: Option<String> = None;
    if should_summarize {
        if let Some(cut) = summarize::find_cut_index(messages, cfg.keep_last_user_turns) {
            let prefix: Vec<_> = messages[..cut].to_vec();
            match summarize::summarize_prefix(
                provider,
                if cfg.model.is_empty() {
                    None
                } else {
                    Some(cfg.model.as_str())
                },
                &prefix,
                cfg.summary_max_tokens,
            )
            .await
            {
                Ok((summary_msg, d)) => {
                    let tail: Vec<_> = messages.drain(cut..).collect();
                    messages.clear();
                    messages.push(summary_msg);
                    messages.extend(tail);
                    summarized = true;
                    digest = Some(d);
                }
                Err(e) => {
                    tracing::warn!("compaction summarize failed (silent fallback): {}", e);
                }
            }
        }
    }

    let strategy = match (truncated > 0, summarized) {
        (true, true) => CompactionStrategy::Both,
        (true, false) => CompactionStrategy::Truncate,
        (false, true) => CompactionStrategy::Summarize,
        (false, false) => CompactionStrategy::Noop,
    };

    CompactionReport {
        reason,
        strategy,
        messages_before,
        messages_after: messages.len(),
        bytes_before,
        bytes_after: total_content_bytes(messages),
        summary_digest: digest,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use llmsh_llm::types::{Message, MessageRole};

    #[test]
    fn total_content_bytes_sums_content_lengths() {
        let m = vec![
            Message {
                role: MessageRole::User,
                content: "abcd".into(),
                tool_call_id: None,
                name: None,
                tool_calls: None,
            },
            Message {
                role: MessageRole::Assistant,
                content: "ef".into(),
                tool_call_id: None,
                name: None,
                tool_calls: None,
            },
        ];
        assert_eq!(total_content_bytes(&m), 6);
    }
}
