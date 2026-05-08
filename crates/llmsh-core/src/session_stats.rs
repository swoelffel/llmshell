use llmsh_llm::context_window::context_window_for;
use llmsh_llm::pricing::pricing_for;
use llmsh_llm::types::{FinishReason, TokenUsage};
use std::time::Duration;

#[derive(Debug, Clone, Default)]
pub struct SessionStats {
    /// Last completed LLM turn (for tier-1 line + status line %).
    pub last_turn: Option<TurnStats>,
    /// Cumulative across the session.
    pub totals: SessionTotals,
}

#[derive(Debug, Clone)]
pub struct TurnStats {
    pub model: String,
    pub input_tokens: u32,
    pub output_tokens: u32,
    pub cached_input_tokens: u32,
    pub finish_reason: FinishReason,
    pub latency: Duration,
    /// Resolved cost in USD; `None` when model has no price entry.
    pub cost_usd: Option<f64>,
    /// Tool steps executed *during* the turn (cleared next turn).
    pub tool_steps: Vec<ToolStepStats>,
    /// Number of schema repair attempts in this turn.
    pub schema_repair_attempts: u32,
}

#[derive(Debug, Clone)]
pub struct ToolStepStats {
    pub tool: String,
    pub status: String,
    pub duration: Duration,
    pub output_bytes: usize,
    pub risk: String,
    pub flags: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct SessionTotals {
    pub turns: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: u64,
    pub cost_usd: f64,
    /// True if any turn used a model without a price entry.
    pub cost_partial: bool,
}

impl SessionStats {
    /// Build a `TurnStats` from a usage payload + model id + latency.
    /// Pure: no I/O. Updates `last_turn` and `totals`.
    pub fn record_turn(
        &mut self,
        model: &str,
        usage: Option<&TokenUsage>,
        finish_reason: FinishReason,
        latency: Duration,
    ) {
        let input = usage.and_then(|u| u.input_tokens).unwrap_or(0);
        let output = usage.and_then(|u| u.output_tokens).unwrap_or(0);
        let cached = usage.and_then(|u| u.cached_input_tokens).unwrap_or(0);
        let cost = pricing_for(model).map(|p| p.cost_usd(input, cached, output));

        self.last_turn = Some(TurnStats {
            model: model.to_string(),
            input_tokens: input,
            output_tokens: output,
            cached_input_tokens: cached,
            finish_reason,
            latency,
            cost_usd: cost,
            tool_steps: Vec::new(),
            schema_repair_attempts: 0,
        });

        self.totals.turns += 1;
        self.totals.input_tokens += input as u64;
        self.totals.output_tokens += output as u64;
        self.totals.cached_input_tokens += cached as u64;
        match cost {
            Some(c) => self.totals.cost_usd += c,
            None => self.totals.cost_partial = true,
        }
    }

    /// Context-window utilization for the last turn (0.0–1.0+).
    pub fn last_context_ratio(&self) -> Option<f64> {
        let t = self.last_turn.as_ref()?;
        let w = context_window_for(&t.model);
        if w == 0 {
            return None;
        }
        Some(t.input_tokens as f64 / w as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: u32, output: u32, cached: u32) -> TokenUsage {
        TokenUsage {
            input_tokens: Some(input),
            output_tokens: Some(output),
            total_tokens: Some(input + output),
            cached_input_tokens: Some(cached),
        }
    }

    #[test]
    fn record_turn_known_model_costs_aggregate() {
        let mut s = SessionStats::default();
        s.record_turn(
            "gpt-4o-mini",
            Some(&usage(1000, 200, 800)),
            FinishReason::Stop,
            Duration::from_millis(1500),
        );
        s.record_turn(
            "gpt-4o-mini",
            Some(&usage(500, 100, 0)),
            FinishReason::Stop,
            Duration::from_millis(900),
        );
        assert_eq!(s.totals.turns, 2);
        assert_eq!(s.totals.input_tokens, 1500);
        assert_eq!(s.totals.output_tokens, 300);
        assert_eq!(s.totals.cached_input_tokens, 800);
        assert!(!s.totals.cost_partial);
        assert!(s.totals.cost_usd > 0.0);
    }

    #[test]
    fn unknown_model_marks_partial_cost() {
        let mut s = SessionStats::default();
        s.record_turn(
            "frob-9000",
            Some(&usage(100, 50, 0)),
            FinishReason::Stop,
            Duration::from_millis(100),
        );
        assert!(s.totals.cost_partial);
        assert_eq!(s.totals.cost_usd, 0.0);
        assert!(s.last_turn.unwrap().cost_usd.is_none());
    }

    #[test]
    fn missing_usage_records_zero() {
        let mut s = SessionStats::default();
        s.record_turn(
            "gpt-4o-mini",
            None,
            FinishReason::Stop,
            Duration::from_millis(50),
        );
        let t = s.last_turn.as_ref().unwrap();
        assert_eq!(t.input_tokens, 0);
        assert_eq!(t.output_tokens, 0);
    }

    #[test]
    fn last_context_ratio_uses_window() {
        let mut s = SessionStats::default();
        s.record_turn(
            "gpt-4o-mini",
            Some(&usage(12_800, 0, 0)),
            FinishReason::Stop,
            Duration::from_millis(10),
        );
        let r = s.last_context_ratio().unwrap();
        assert!((r - 0.10).abs() < 1e-9, "got {}", r);
    }
}
