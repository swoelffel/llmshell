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

use crate::config::{CompactConfig, MemoryConfig};
use crate::memory::{ClearSource, ConversationMessage, Memory};
use llmsh_llm::context_window::context_window_for;
use llmsh_llm::provider::LlmProvider;
use std::sync::Arc;

/// Run the cascade (stage A then optionally B) on `messages` in place.
///
/// `last_input_tokens` is the prompt_tokens of the most recent provider
/// response — used to decide whether to trigger stage B for the auto path.
/// For the manual path, pass `u32::MAX` to force stage B.
#[allow(clippy::too_many_arguments)]
pub async fn compact(
    messages: &mut Vec<llmsh_llm::types::Message>,
    cfg: &CompactConfig,
    memory_cfg: &MemoryConfig,
    reason: CompactionReason,
    model: &str,
    last_input_tokens: u32,
    provider: Arc<dyn LlmProvider>,
    memory: Arc<Memory>,
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

            let current_facts: Vec<(String, String)> = memory
                .load_active_facts()
                .unwrap_or_default()
                .into_iter()
                .map(|f| (f.category, f.claim))
                .collect();

            match summarize::summarize_and_extract(
                provider,
                if cfg.model.is_empty() {
                    None
                } else {
                    Some(cfg.model.as_str())
                },
                &prefix,
                &current_facts,
                cfg.summary_max_tokens,
                memory_cfg.max_facts,
            )
            .await
            {
                Ok((summary_msg, facts, d)) => {
                    let ts =
                        chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true);

                    // ──────────────────────────────────────────────────────
                    // SQLite/RAM compaction dance:
                    //   1. Mark all active conversation_messages cleared
                    //      (cleared_source='compact').
                    //   2. INSERT the summary message (insert_source='compact').
                    //   3. Re-INSERT each tail message (insert_source='compact_tail')
                    //      so load_active_conversation() returns [summary, ...tail].
                    //   4. replace_facts_generation with new curated facts.
                    // The audit log retains everything (cleared rows are still rows).
                    // ──────────────────────────────────────────────────────

                    let _ = memory.mark_conversation_cleared(&ts, ClearSource::Compact);

                    let _ = memory.append_message(&ConversationMessage {
                        id: 0,
                        ts: ts.clone(),
                        role: "assistant".into(),
                        content: summary_msg.content.clone(),
                        tool_call_id: None,
                        name: None,
                        tool_calls_json: None,
                        insert_source: "compact".into(),
                    });

                    let tail = messages[cut..].to_vec();
                    for m in &tail {
                        let role = match m.role {
                            llmsh_llm::types::MessageRole::User => "user",
                            llmsh_llm::types::MessageRole::Assistant => "assistant",
                            llmsh_llm::types::MessageRole::Tool => "tool",
                            llmsh_llm::types::MessageRole::System => "system",
                        };
                        let tcs = m
                            .tool_calls
                            .as_ref()
                            .and_then(|tcs| serde_json::to_string(tcs).ok());
                        let _ = memory.append_message(&ConversationMessage {
                            id: 0,
                            ts: ts.clone(),
                            role: role.into(),
                            content: m.content.clone(),
                            tool_call_id: m.tool_call_id.clone(),
                            name: m.name.clone(),
                            tool_calls_json: tcs,
                            insert_source: "compact_tail".into(),
                        });
                    }

                    let new_facts: Vec<(String, String)> = facts
                        .iter()
                        .map(|f| (f.category.clone(), f.claim.clone()))
                        .collect();
                    let _ = memory.replace_facts_generation(&ts, &new_facts);

                    // RAM update.
                    messages.clear();
                    messages.push(summary_msg);
                    messages.extend(tail);

                    summarized = true;
                    digest = Some(d);
                }
                Err(e) => {
                    tracing::warn!(
                        "compaction summarize+extract failed (silent fallback): {}",
                        e
                    );
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
