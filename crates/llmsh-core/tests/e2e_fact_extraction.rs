//! End-to-end coverage for fact extraction via `compactor::compact()`.
//!
//! Exercises the full summarize+extract path: ScriptedProvider returns valid
//! JSON `{"summary":"…","facts":[…]}` and the results are persisted to the
//! in-memory `Memory` store.

use async_trait::async_trait;
use llmsh_core::compactor::{self, CompactionReason, CompactionStrategy};
use llmsh_core::config::{CompactConfig, MemoryConfig};
use llmsh_core::memory::Memory;
use llmsh_llm::capabilities::{Capabilities, ToolCallingMode};
use llmsh_llm::provider::LlmProvider;
use llmsh_llm::types::{FinishReason, LlmRequest, LlmResponse, Message, MessageRole, ModelInfo};
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------------------
// ScriptedProvider — same pattern as e2e_compaction.rs
// ---------------------------------------------------------------------------

struct ScriptedProvider {
    responses: Mutex<Vec<LlmResponse>>,
}

#[async_trait]
impl LlmProvider for ScriptedProvider {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            tool_calling: ToolCallingMode::Native,
            supports_streaming: false,
            supports_json_mode: true,
            supports_parallel_tool_calls: true,
            supports_tool_choice_required: true,
            max_context_tokens: None,
        }
    }
    async fn complete(&self, _: LlmRequest) -> anyhow::Result<LlmResponse> {
        Ok(self.responses.lock().unwrap().remove(0))
    }
    async fn list_models(&self) -> anyhow::Result<Vec<ModelInfo>> {
        Ok(vec![])
    }
    async fn set_model(&self, _: &str) -> anyhow::Result<()> {
        Ok(())
    }
    fn current_model(&self) -> String {
        "mock:test".into()
    }
}

fn stop_json(json: &str) -> LlmResponse {
    LlmResponse {
        message: Some(json.into()),
        tool_calls: vec![],
        finish_reason: FinishReason::Stop,
        usage: None,
    }
}

fn user(s: &str) -> Message {
    Message {
        role: MessageRole::User,
        content: s.into(),
        tool_call_id: None,
        name: None,
        tool_calls: None,
    }
}

fn assistant(s: &str) -> Message {
    Message {
        role: MessageRole::Assistant,
        content: s.into(),
        tool_call_id: None,
        name: None,
        tool_calls: None,
    }
}

/// Build a message list with enough user turns to fire `find_cut_index` for
/// `keep_last_user_turns = 2`.  Needs ≥ 3 user messages.
fn enough_messages() -> Vec<Message> {
    vec![
        user("je suis alice"),
        assistant("ok"),
        user("what is rust?"),
        assistant("a systems language"),
        user("thanks"),
        assistant("you're welcome"),
    ]
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// First compact: empty DB → 1 fact in generation 1.
#[tokio::test]
async fn first_compact_populates_facts_generation_1() {
    let mem = Arc::new(Memory::open_in_memory().unwrap());
    let provider: Arc<dyn LlmProvider> = Arc::new(ScriptedProvider {
        responses: Mutex::new(vec![stop_json(
            r#"{"summary":"alice introduced herself","facts":[{"category":"identity","claim":"user is alice"}]}"#,
        )]),
    });

    let cfg = CompactConfig {
        auto_threshold_pct: 0,
        keep_last_user_turns: 2,
        tool_output_max_bytes: 2048,
        summary_max_tokens: 200,
        model: String::new(),
    };
    let mem_cfg = MemoryConfig::default();

    let mut messages = enough_messages();
    let report = compactor::compact(
        &mut messages,
        &cfg,
        &mem_cfg,
        CompactionReason::Manual,
        "openai:gpt-4o-mini",
        u32::MAX,
        provider,
        mem.clone(),
    )
    .await;

    // Compaction must have run the summarize stage.
    assert!(
        matches!(
            report.strategy,
            CompactionStrategy::Summarize | CompactionStrategy::Both
        ),
        "expected Summarize or Both strategy, got {:?}",
        report.strategy
    );

    let facts = mem.load_active_facts().unwrap();
    assert_eq!(facts.len(), 1, "exactly one fact must be extracted");
    assert!(
        facts[0].claim.contains("alice"),
        "fact claim must mention alice; got: {}",
        facts[0].claim
    );
    assert_eq!(
        facts[0].generation, 1,
        "first compaction must write generation 1"
    );
    assert_eq!(facts[0].insert_source, "compact");
}

/// Second compact: provider returns a different facts list. Generation must
/// increment to 2; the old generation-1 rows are NOT soft-deleted by
/// `replace_facts_generation` — `load_active_facts` filters by MAX(generation).
#[tokio::test]
async fn second_compact_bumps_generation_and_old_rows_remain() {
    let mem = Arc::new(Memory::open_in_memory().unwrap());

    // First compact.
    {
        let provider: Arc<dyn LlmProvider> = Arc::new(ScriptedProvider {
            responses: Mutex::new(vec![stop_json(
                r#"{"summary":"first summary","facts":[{"category":"identity","claim":"user is alice"}]}"#,
            )]),
        });
        let cfg = CompactConfig {
            auto_threshold_pct: 0,
            keep_last_user_turns: 2,
            tool_output_max_bytes: 2048,
            summary_max_tokens: 200,
            model: String::new(),
        };
        let mut messages = enough_messages();
        compactor::compact(
            &mut messages,
            &cfg,
            &MemoryConfig::default(),
            CompactionReason::Manual,
            "openai:gpt-4o-mini",
            u32::MAX,
            provider,
            mem.clone(),
        )
        .await;
    }

    // Verify generation 1 landed.
    let facts_g1 = mem.load_active_facts().unwrap();
    assert_eq!(facts_g1.len(), 1);
    assert_eq!(facts_g1[0].generation, 1);

    // Second compact — provider returns an updated facts list.
    {
        let provider: Arc<dyn LlmProvider> = Arc::new(ScriptedProvider {
            responses: Mutex::new(vec![stop_json(
                r#"{"summary":"second summary","facts":[{"category":"identity","claim":"user is alice (confirmed)"},{"category":"preference","claim":"likes Rust"}]}"#,
            )]),
        });
        let cfg = CompactConfig {
            auto_threshold_pct: 0,
            keep_last_user_turns: 2,
            tool_output_max_bytes: 2048,
            summary_max_tokens: 200,
            model: String::new(),
        };
        // Rebuild a fresh enough_messages list (previous compact replaced RAM messages).
        let mut messages = enough_messages();
        let report = compactor::compact(
            &mut messages,
            &cfg,
            &MemoryConfig::default(),
            CompactionReason::Manual,
            "openai:gpt-4o-mini",
            u32::MAX,
            provider,
            mem.clone(),
        )
        .await;

        assert!(
            matches!(
                report.strategy,
                CompactionStrategy::Summarize | CompactionStrategy::Both
            ),
            "second compact must also summarize"
        );
    }

    // Active facts must now be generation 2 with the new claims.
    let facts_g2 = mem.load_active_facts().unwrap();
    assert!(
        facts_g2.iter().any(|f| f.generation == 2),
        "generation 2 facts must be present"
    );
    assert!(
        facts_g2
            .iter()
            .any(|f| f.claim.contains("confirmed") || f.claim.contains("alice")),
        "updated alice claim must appear in generation 2"
    );
    assert!(
        facts_g2.iter().any(|f| f.claim.contains("Rust")),
        "new Rust preference fact must appear in generation 2"
    );

    // Generation-1 rows must still exist in the DB (not soft-deleted by
    // replace_facts_generation). The total generation count is ≥ 2.
    let gen = mem.current_fact_generation().unwrap();
    assert_eq!(gen, 2, "current_fact_generation must return 2");
}
