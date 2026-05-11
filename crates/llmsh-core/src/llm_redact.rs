//! Redaction at the LLM boundary — delegates to `llmsh_redact`.
//!
//! Applied to outbound prompts/tool outputs that would otherwise leak local
//! secrets into the model context.

use llmsh_redact::{default_patterns, PatternDef};
use regex::Regex;

pub struct LlmRedactor {
    patterns: Vec<(String, Regex)>,
}

impl Default for LlmRedactor {
    fn default() -> Self {
        Self::from_defs(&default_patterns())
    }
}

impl LlmRedactor {
    fn from_defs(defs: &[PatternDef]) -> Self {
        let patterns = defs
            .iter()
            .map(|d| {
                let re = Regex::new(d.regex)
                    .unwrap_or_else(|e| panic!("redact pattern {} invalid: {e}", d.name));
                (d.name.to_string(), re)
            })
            .collect();
        Self { patterns }
    }

    pub fn redact(&self, s: &str) -> (String, usize) {
        let mut hits = 0;
        let mut out = s.to_string();
        for (name, re) in &self.patterns {
            out = re
                .replace_all(&out, |_: &regex::Captures| {
                    hits += 1;
                    format!("[REDACTED:{}]", name)
                })
                .into_owned();
        }
        (out, hits)
    }

    pub fn truncate(&self, s: &str, max: usize) -> (String, bool) {
        if s.len() <= max {
            (s.to_string(), false)
        } else {
            (format!("{}…[truncated]", &s[..max]), true)
        }
    }
}

/// Convenience free function used at the persistence and HTTP-error boundaries.
pub fn redact_for_llm(input: &str) -> String {
    LlmRedactor::default().redact(input).0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_dotenv() {
        let out = redact_for_llm("API_TOKEN=abc123def456");
        assert!(out.contains("[REDACTED:dotenv_secret]"));
    }

    #[test]
    fn idempotent() {
        let s = "sk-ant-api03-AAAAAAAAAAAAAAAAAAAA";
        let once = redact_for_llm(s);
        assert_eq!(once, redact_for_llm(&once));
    }
}
