//! Asserts the format of the tier-1 stderr line for a scripted single turn.

use llmsh_core::session_stats::SessionStats;
use llmsh_core::verbose_print::print_turn_verbose;
use llmsh_llm::types::{FinishReason, TokenUsage};
use std::time::Duration;

#[test]
fn tier1_line_matches_expected_shape() {
    let mut s = SessionStats::default();
    s.record_turn(
        "gpt-4o-mini",
        Some(&TokenUsage {
            input_tokens: Some(1234),
            output_tokens: Some(156),
            total_tokens: Some(1390),
            cached_input_tokens: Some(892),
        }),
        FinishReason::Stop,
        Duration::from_millis(1800),
    );
    let mut buf: Vec<u8> = Vec::new();
    print_turn_verbose(&mut buf, &s, 1);
    let line = String::from_utf8(buf).unwrap();

    // Format contract: `↳ <in> in (<cached> cached, <pct>%) · <out> out · $<cost> · <s>s · <model> · <reason>`
    assert!(line.starts_with("↳ 1234 in (892 cached, 72%)"), "{}", line);
    assert!(line.contains("· 156 out ·"), "{}", line);
    assert!(line.contains("· 1.8s ·"), "{}", line);
    assert!(line.ends_with("gpt-4o-mini · stop\n"), "{}", line);
    assert!(line.contains("$"), "{}", line);
}

#[test]
fn level_zero_emits_nothing() {
    let mut s = SessionStats::default();
    s.record_turn(
        "gpt-4o-mini",
        None,
        FinishReason::Stop,
        Duration::from_millis(10),
    );
    let mut buf: Vec<u8> = Vec::new();
    print_turn_verbose(&mut buf, &s, 0);
    assert!(buf.is_empty());
}
