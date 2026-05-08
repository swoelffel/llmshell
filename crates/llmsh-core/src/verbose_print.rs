use crate::session_stats::{SessionStats, ToolStepStats, TurnStats};
use std::io::Write;

/// Print tier-1 (and optionally tier-2) lines for the most recently completed
/// turn. Writes to stderr. Returns silently if stats are missing.
pub fn print_turn_verbose<W: Write>(out: &mut W, stats: &SessionStats, level: u8) {
    if level == 0 {
        return;
    }
    let Some(t) = stats.last_turn.as_ref() else {
        return;
    };
    let _ = writeln!(out, "{}", format_tier1(t));
    if level >= 2 {
        for step in &t.tool_steps {
            let _ = writeln!(out, "{}", format_tier2_step(step));
        }
        if t.schema_repair_attempts > 0 {
            let _ = writeln!(
                out,
                "↺ schema repair: {} attempts",
                t.schema_repair_attempts
            );
        }
    }
}

pub fn format_tier1(t: &TurnStats) -> String {
    let cached_pct = if t.input_tokens == 0 {
        0
    } else {
        ((t.cached_input_tokens as f64 / t.input_tokens as f64) * 100.0).round() as u32
    };
    let cost = match t.cost_usd {
        Some(c) => format!("${:.4}", c),
        None => "$?".to_string(),
    };
    format!(
        "↳ {} in ({} cached, {}%) · {} out · {} · {:.1}s · {} · {}",
        t.input_tokens,
        t.cached_input_tokens,
        cached_pct,
        t.output_tokens,
        cost,
        t.latency.as_secs_f64(),
        t.model,
        finish_reason_label(t.finish_reason),
    )
}

pub fn format_tier2_step(s: &ToolStepStats) -> String {
    let bytes = human_bytes(s.output_bytes);
    let mut line = format!(
        "  · {} · {} ms · {} · risk={}",
        s.tool,
        s.duration.as_millis(),
        bytes,
        s.risk,
    );
    if !s.flags.is_empty() {
        line.push_str(" · flags=");
        line.push_str(&s.flags.join(","));
    }
    let _ = &s.status; // status currently implicit (success path). reserved for future surfacing.
    line
}

fn finish_reason_label(r: llmsh_llm::types::FinishReason) -> &'static str {
    use llmsh_llm::types::FinishReason::*;
    match r {
        Stop => "stop",
        ToolCalls => "tool_calls",
        Length => "length",
        Refusal => "refusal",
        Error => "error",
    }
}

fn human_bytes(n: usize) -> String {
    if n >= 1024 * 1024 {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    } else if n >= 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{} B", n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_stats::SessionStats;
    use llmsh_llm::types::{FinishReason, TokenUsage};
    use std::time::Duration;

    fn stats_with_turn() -> SessionStats {
        let mut s = SessionStats::default();
        let usage = TokenUsage {
            input_tokens: Some(1234),
            output_tokens: Some(156),
            total_tokens: Some(1390),
            cached_input_tokens: Some(892),
        };
        s.record_turn(
            "gpt-4o-mini",
            Some(&usage),
            FinishReason::Stop,
            Duration::from_millis(1800),
        );
        s
    }

    #[test]
    fn level_zero_prints_nothing() {
        let s = stats_with_turn();
        let mut buf = Vec::new();
        print_turn_verbose(&mut buf, &s, 0);
        assert!(buf.is_empty());
    }

    #[test]
    fn level_one_prints_tier1_only() {
        let s = stats_with_turn();
        let mut buf = Vec::new();
        print_turn_verbose(&mut buf, &s, 1);
        let out = String::from_utf8(buf).unwrap();
        assert!(out.starts_with("↳ 1234 in (892 cached, 72%)"), "{}", out);
        assert!(out.contains("156 out"), "{}", out);
        assert!(out.contains("1.8s"), "{}", out);
        assert!(out.contains("gpt-4o-mini"), "{}", out);
        assert!(out.contains("stop"), "{}", out);
        assert_eq!(out.lines().count(), 1);
    }

    #[test]
    fn level_two_prints_tool_steps() {
        let mut s = stats_with_turn();
        if let Some(t) = s.last_turn.as_mut() {
            t.tool_steps.push(ToolStepStats {
                tool: "read_file".into(),
                status: "success".into(),
                duration: Duration::from_millis(12),
                output_bytes: 4300,
                risk: "ReadOnly".into(),
                flags: vec![],
            });
            t.schema_repair_attempts = 2;
        }
        let mut buf = Vec::new();
        print_turn_verbose(&mut buf, &s, 2);
        let out = String::from_utf8(buf).unwrap();
        assert!(out.contains("↳ 1234 in"), "{}", out);
        assert!(out.contains("· read_file · 12 ms"), "{}", out);
        assert!(out.contains("risk=ReadOnly"), "{}", out);
        assert!(out.contains("schema repair: 2 attempts"), "{}", out);
    }

    #[test]
    fn unknown_cost_renders_question_mark() {
        let mut s = SessionStats::default();
        let usage = TokenUsage {
            input_tokens: Some(10),
            output_tokens: Some(2),
            total_tokens: Some(12),
            cached_input_tokens: Some(0),
        };
        s.record_turn(
            "frob-9000",
            Some(&usage),
            FinishReason::Stop,
            Duration::from_millis(10),
        );
        let line = format_tier1(s.last_turn.as_ref().unwrap());
        assert!(line.contains("$?"), "{}", line);
    }
}
