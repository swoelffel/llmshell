use crate::session_stats::SessionStats;
use llmsh_llm::context_window::context_window_for;
use nu_ansi_term::{Color, Style};
use reedline::{Prompt, PromptEditMode, PromptHistorySearch, PromptHistorySearchStatus};
use std::borrow::Cow;
use std::sync::{Arc, RwLock};

/// Reedline prompt that prepends a one-line session status:
/// `[model · in/window (pct%) · $cost] > `
pub struct StatusPrompt {
    pub model: Arc<RwLock<String>>,
    pub stats: Arc<RwLock<SessionStats>>,
    pub colored: bool,
}

impl StatusPrompt {
    pub fn new(
        model: Arc<RwLock<String>>,
        stats: Arc<RwLock<SessionStats>>,
        colored: bool,
    ) -> Self {
        Self {
            model,
            stats,
            colored,
        }
    }

    fn render(&self) -> String {
        let model = self
            .model
            .read()
            .map(|g| g.clone())
            .unwrap_or_else(|_| "unknown".into());
        let snap = self.stats.read().map(|s| s.clone()).unwrap_or_default();

        let window = context_window_for(&model);
        let (last_in, ratio) = match snap.last_turn.as_ref() {
            Some(t) => {
                let r = if window > 0 {
                    t.input_tokens as f64 / window as f64
                } else {
                    0.0
                };
                (t.input_tokens, r)
            }
            None => (0, 0.0),
        };

        let cost_str = if snap.totals.turns == 0 {
            "$0.0000".to_string()
        } else if snap.totals.cost_partial && snap.totals.cost_usd == 0.0 {
            "$?".to_string()
        } else if snap.totals.cost_partial {
            format!("${:.4}+", snap.totals.cost_usd)
        } else {
            format!("${:.4}", snap.totals.cost_usd)
        };

        let pct = (ratio * 100.0).round() as u32;
        let body = format!(
            "[{} · {}/{} ({}%) · {}]",
            model,
            human_tokens(last_in),
            human_tokens(window),
            pct,
            cost_str
        );

        if self.colored {
            let style = ratio_style(ratio);
            style.paint(body).to_string()
        } else {
            body
        }
    }
}

fn ratio_style(ratio: f64) -> Style {
    if ratio > 0.90 {
        Style::new().fg(Color::Red).bold()
    } else if ratio > 0.70 {
        Style::new().fg(Color::Yellow)
    } else {
        Style::new().fg(Color::DarkGray)
    }
}

fn human_tokens(n: u32) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

impl Prompt for StatusPrompt {
    fn render_prompt_left(&self) -> Cow<'_, str> {
        Cow::Owned(self.render())
    }
    fn render_prompt_right(&self) -> Cow<'_, str> {
        Cow::Borrowed("")
    }
    fn render_prompt_indicator(&self, _: PromptEditMode) -> Cow<'_, str> {
        Cow::Borrowed(" > ")
    }
    fn render_prompt_multiline_indicator(&self) -> Cow<'_, str> {
        Cow::Borrowed("::: ")
    }
    fn render_prompt_history_search_indicator(
        &self,
        history_search: PromptHistorySearch,
    ) -> Cow<'_, str> {
        let prefix = match history_search.status {
            PromptHistorySearchStatus::Passing => "",
            PromptHistorySearchStatus::Failing => "failing ",
        };
        Cow::Owned(format!(
            "({}reverse-search: {}) ",
            prefix, history_search.term
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use llmsh_llm::types::{FinishReason, TokenUsage};
    use std::time::Duration;

    fn make() -> (StatusPrompt, Arc<RwLock<SessionStats>>) {
        let model = Arc::new(RwLock::new("gpt-4o-mini".to_string()));
        let stats = Arc::new(RwLock::new(SessionStats::default()));
        (StatusPrompt::new(model, stats.clone(), false), stats)
    }

    #[test]
    fn fresh_session_renders_zero_cost() {
        let (p, _) = make();
        let out = p.render();
        assert!(out.contains("gpt-4o-mini"), "{}", out);
        assert!(out.contains("0/128.0k"), "{}", out);
        assert!(out.contains("$0.0000"), "{}", out);
    }

    #[test]
    fn after_turn_shows_input_and_cost() {
        let (p, stats) = make();
        let usage = TokenUsage {
            input_tokens: Some(12_800),
            output_tokens: Some(100),
            total_tokens: Some(12_900),
            cached_input_tokens: Some(0),
        };
        stats.write().unwrap().record_turn(
            "gpt-4o-mini",
            Some(&usage),
            FinishReason::Stop,
            Duration::from_millis(500),
        );
        let out = p.render();
        assert!(out.contains("12.8k/128.0k"), "{}", out);
        assert!(out.contains("(10%)"), "{}", out);
        assert!(out.contains("$0.0"), "{}", out);
    }

    #[test]
    fn unknown_model_cost_marker() {
        let model = Arc::new(RwLock::new("frob-9000".to_string()));
        let stats = Arc::new(RwLock::new(SessionStats::default()));
        let p = StatusPrompt::new(model, stats.clone(), false);
        let usage = TokenUsage {
            input_tokens: Some(10),
            output_tokens: Some(5),
            total_tokens: Some(15),
            cached_input_tokens: Some(0),
        };
        stats.write().unwrap().record_turn(
            "frob-9000",
            Some(&usage),
            FinishReason::Stop,
            Duration::from_millis(10),
        );
        assert!(p.render().contains("$?"), "{}", p.render());
    }
}
