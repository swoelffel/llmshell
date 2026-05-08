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
    /// Begin a fresh user-input turn: clears any in-progress tool steps and
    /// schema-repair counters. Call once at the start of `AgentLoop::run`,
    /// before the LLM-iteration loop.
    pub fn begin_user_turn(&mut self) {
        if let Some(t) = self.last_turn.as_mut() {
            t.tool_steps.clear();
            t.schema_repair_attempts = 0;
        }
    }

    /// Build a `TurnStats` from a usage payload + model id + latency.
    /// Pure: no I/O. Updates `last_turn` and `totals`.
    /// Within a single user turn (across multiple LLM iterations), the
    /// `tool_steps` and `schema_repair_attempts` accumulators carry over
    /// from the prior iteration's `last_turn`. Use `begin_user_turn` to
    /// reset them at the start of a new user input.
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

        let (carry_steps, carry_attempts) = self
            .last_turn
            .as_ref()
            .map(|t| (t.tool_steps.clone(), t.schema_repair_attempts))
            .unwrap_or_else(|| (Vec::new(), 0));

        self.last_turn = Some(TurnStats {
            model: model.to_string(),
            input_tokens: input,
            output_tokens: output,
            cached_input_tokens: cached,
            finish_reason,
            latency,
            cost_usd: cost,
            tool_steps: carry_steps,
            schema_repair_attempts: carry_attempts,
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

    #[test]
    fn tool_steps_carry_across_iterations_until_begin_user_turn() {
        let mut s = SessionStats::default();
        // Iteration 1: provider responds with tool_calls, then we push 2 steps.
        s.record_turn(
            "gpt-4o-mini",
            Some(&usage(100, 50, 0)),
            FinishReason::ToolCalls,
            Duration::from_millis(100),
        );
        if let Some(t) = s.last_turn.as_mut() {
            t.tool_steps.push(ToolStepStats {
                tool: "list_directory".into(),
                status: "success".into(),
                duration: Duration::from_millis(5),
                output_bytes: 10,
                risk: "ReadOnly".into(),
                flags: vec![],
            });
        }
        // Iteration 2: another tool_calls round, push 1 step.
        s.record_turn(
            "gpt-4o-mini",
            Some(&usage(200, 80, 0)),
            FinishReason::ToolCalls,
            Duration::from_millis(120),
        );
        if let Some(t) = s.last_turn.as_mut() {
            t.tool_steps.push(ToolStepStats {
                tool: "read_file".into(),
                status: "success".into(),
                duration: Duration::from_millis(8),
                output_bytes: 1024,
                risk: "ReadOnly".into(),
                flags: vec![],
            });
        }
        // Iteration 3: Stop. record_turn must preserve the previous 2 steps.
        s.record_turn(
            "gpt-4o-mini",
            Some(&usage(300, 100, 0)),
            FinishReason::Stop,
            Duration::from_millis(80),
        );
        let t = s.last_turn.as_ref().unwrap();
        assert_eq!(
            t.tool_steps.len(),
            2,
            "expected accumulated steps, got: {:?}",
            t.tool_steps
        );
        assert_eq!(t.tool_steps[0].tool, "list_directory");
        assert_eq!(t.tool_steps[1].tool, "read_file");

        // Now begin a fresh user turn — steps must reset.
        s.begin_user_turn();
        let t2 = s.last_turn.as_ref().unwrap();
        assert!(
            t2.tool_steps.is_empty(),
            "begin_user_turn must clear tool_steps"
        );
        assert_eq!(t2.schema_repair_attempts, 0);
    }
}
