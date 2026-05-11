//! Audit redaction façade — delegates to `llmsh_redact`.
//!
//! Preserves the legacy `Redactor::default_audit()` + `redact(&str) -> (String, usize)`
//! API so call-sites compile unchanged. Internal patterns are now the canonical
//! set from `llmsh-redact`.

use llmsh_redact::{default_patterns, PatternDef};
use regex::Regex;

pub struct Redactor {
    patterns: Vec<(String, Regex)>,
}

impl Redactor {
    pub fn default_audit() -> Self {
        Self::from_defs(&default_patterns())
    }

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

    pub fn redact(&self, input: &str) -> (String, usize) {
        let mut hits = 0usize;
        let mut out = input.to_string();
        for (name, re) in &self.patterns {
            let replaced = re.replace_all(&out, |_caps: &regex::Captures| {
                hits += 1;
                format!("[REDACTED:{}]", name)
            });
            out = replaced.into_owned();
        }
        (out, hits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_openai_key() {
        let r = Redactor::default_audit();
        let (out, hits) = r.redact("token=sk-abcdefghijklmnopqrstuv tail");
        assert_eq!(hits, 1);
        assert!(out.contains("[REDACTED:openai_key]"));
        assert!(!out.contains("sk-abcdefghij"));
    }

    #[test]
    fn redacts_multiline_pem() {
        let r = Redactor::default_audit();
        let key = "-----BEGIN RSA PRIVATE KEY-----\nMIIB\nXYZ\n-----END RSA PRIVATE KEY-----";
        let (out, hits) = r.redact(&format!("blob {} after", key));
        assert_eq!(hits, 1);
        assert!(out.contains("[REDACTED:pem_private_key]"));
        assert!(!out.contains("MIIB"));
    }

    #[test]
    fn does_not_match_normal_text() {
        let r = Redactor::default_audit();
        let (_, hits) = r.redact("just a normal sentence");
        assert_eq!(hits, 0);
    }
}
