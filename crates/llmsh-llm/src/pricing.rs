//! Per-model price table ($ per 1M tokens). May 2026 OpenAI + Anthropic lineups.
//! Source values: provider public pricing pages (OpenAI verified for v0.2.2;
//! Anthropic Claude 4.x added for v0.3.0).
//! Returns `None` for unknown models so the UI can render `?` rather than a
//! misleading 0.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPricing {
    /// USD per 1M input tokens.
    pub input_per_million: f64,
    /// USD per 1M cached input tokens.
    pub cached_input_per_million: f64,
    /// USD per 1M output tokens.
    pub output_per_million: f64,
}

impl ModelPricing {
    /// Cost of a single turn in USD given (input, cached, output) token counts.
    /// `cached` must be ≤ `input` (uncached input = `input - cached`).
    pub fn cost_usd(&self, input: u32, cached: u32, output: u32) -> f64 {
        let cached = cached.min(input);
        let uncached = input - cached;
        (uncached as f64) * self.input_per_million / 1_000_000.0
            + (cached as f64) * self.cached_input_per_million / 1_000_000.0
            + (output as f64) * self.output_per_million / 1_000_000.0
    }
}

/// Lookup price for a model id (with optional `provider:` prefix).
pub fn pricing_for(model: &str) -> Option<ModelPricing> {
    let m = strip_provider_prefix(model);
    Some(match m {
        // GPT-5.x flagship (cached at 10% of input)
        "gpt-5.5" | "chat-latest" => ModelPricing {
            input_per_million: 5.00,
            cached_input_per_million: 0.50,
            output_per_million: 30.00,
        },
        "gpt-5.5-pro" => ModelPricing {
            input_per_million: 30.00,
            cached_input_per_million: 30.00, // no cached discount documented
            output_per_million: 180.00,
        },
        "gpt-5.4" => ModelPricing {
            input_per_million: 2.50,
            cached_input_per_million: 0.25,
            output_per_million: 15.00,
        },
        "gpt-5.4-mini" => ModelPricing {
            input_per_million: 0.75,
            cached_input_per_million: 0.075,
            output_per_million: 4.50,
        },
        "gpt-5.4-nano" => ModelPricing {
            input_per_million: 0.20,
            cached_input_per_million: 0.02,
            output_per_million: 1.25,
        },
        // Legacy (cached ≈ 50% input)
        "gpt-4.1" => ModelPricing {
            input_per_million: 2.00,
            cached_input_per_million: 0.50,
            output_per_million: 8.00,
        },
        "gpt-4.1-mini" => ModelPricing {
            input_per_million: 0.40,
            cached_input_per_million: 0.10,
            output_per_million: 1.60,
        },
        "gpt-4.1-nano" => ModelPricing {
            input_per_million: 0.10,
            cached_input_per_million: 0.025,
            output_per_million: 0.40,
        },
        "gpt-4o" => ModelPricing {
            input_per_million: 2.50,
            cached_input_per_million: 1.25,
            output_per_million: 10.00,
        },
        "gpt-4o-mini" => ModelPricing {
            input_per_million: 0.15,
            cached_input_per_million: 0.075,
            output_per_million: 0.60,
        },
        "o3" | "o1" => ModelPricing {
            input_per_million: 15.00,
            cached_input_per_million: 7.50,
            output_per_million: 60.00,
        },
        // Anthropic Claude 4.x (cached input billed at 10% of base, prompt-cache hits).
        "claude-haiku-4-5" => ModelPricing {
            input_per_million: 1.00,
            cached_input_per_million: 0.10,
            output_per_million: 5.00,
        },
        "claude-sonnet-4-6" => ModelPricing {
            input_per_million: 3.00,
            cached_input_per_million: 0.30,
            output_per_million: 15.00,
        },
        "claude-opus-4-7" => ModelPricing {
            input_per_million: 15.00,
            cached_input_per_million: 1.50,
            output_per_million: 75.00,
        },
        _ => return None,
    })
}

fn strip_provider_prefix(m: &str) -> &str {
    m.split_once(':').map(|(_, r)| r).unwrap_or(m)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_model_returns_some() {
        let p = pricing_for("gpt-4o-mini").unwrap();
        assert_eq!(p.input_per_million, 0.15);
        assert_eq!(p.cached_input_per_million, 0.075);
        assert_eq!(p.output_per_million, 0.60);
    }

    #[test]
    fn claude_pricing_lookup() {
        let h = pricing_for("claude-haiku-4-5").unwrap();
        assert_eq!(h.input_per_million, 1.00);
        assert_eq!(h.cached_input_per_million, 0.10);
        assert_eq!(h.output_per_million, 5.00);

        let o = pricing_for("anthropic:claude-opus-4-7").unwrap();
        assert_eq!(o.input_per_million, 15.00);
        assert_eq!(o.output_per_million, 75.00);

        assert!(pricing_for("claude-sonnet-4-6").is_some());
    }

    #[test]
    fn unknown_model_returns_none() {
        assert!(pricing_for("frob-9000").is_none());
    }

    #[test]
    fn cost_excludes_cached_from_uncached() {
        let p = pricing_for("gpt-4o-mini").unwrap();
        // 1000 input, 800 cached, 200 output
        // uncached = 200 → 200 * 0.15 / 1e6 = 0.00003
        // cached   = 800 → 800 * 0.075 / 1e6 = 0.00006
        // output   = 200 → 200 * 0.60 / 1e6 = 0.00012
        let c = p.cost_usd(1000, 800, 200);
        assert!((c - 0.00021).abs() < 1e-9, "got {}", c);
    }

    #[test]
    fn cached_capped_at_input() {
        let p = pricing_for("gpt-4o-mini").unwrap();
        // cached > input is treated as cached = input
        let c = p.cost_usd(100, 999, 0);
        assert!((c - (100.0 * 0.075 / 1e6)).abs() < 1e-12);
    }

    #[test]
    fn provider_prefix_stripped() {
        assert!(pricing_for("openai:gpt-4o-mini").is_some());
    }
}
