use regex::Regex;

pub struct LlmRedactor {
    patterns: Vec<(String, Regex)>,
}

impl LlmRedactor {
    pub fn default() -> Self {
        // Stricter than audit (LLM output is exfiltrated to a third party).
        let raw = [
            ("openai_key", r"sk-[A-Za-z0-9]{20,}"),
            ("anthropic_key", r"sk-ant-[A-Za-z0-9-]{20,}"),
            ("github_token", r"gh[pousr]_[A-Za-z0-9]{20,}"),
            ("aws_access_key", r"AKIA[0-9A-Z]{16}"),
            ("gcp_key", r"AIza[0-9A-Za-z_\-]{20,}"),
            (
                "jwt",
                r"eyJ[A-Za-z0-9_-]+\.eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+",
            ),
            ("bearer_token", r"Bearer\s+[A-Za-z0-9._\-]{20,}"),
            (
                "pem_private_key",
                r"(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?-----END [A-Z ]*PRIVATE KEY-----",
            ),
        ];
        let patterns = raw
            .iter()
            .map(|(n, p)| (n.to_string(), Regex::new(p).unwrap()))
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
