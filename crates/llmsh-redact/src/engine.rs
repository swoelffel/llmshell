use crate::patterns::{default_patterns, PatternDef};
use regex::Regex;

pub struct Redactor {
    rules: Vec<(String, Regex)>,
}

impl Redactor {
    pub fn new(defs: &[PatternDef]) -> Self {
        let rules = defs
            .iter()
            .map(|d| {
                let re = Regex::new(d.regex)
                    .unwrap_or_else(|e| panic!("redact pattern {} invalid: {e}", d.name));
                (d.name.to_string(), re)
            })
            .collect();
        Self { rules }
    }

    pub fn redact(&self, input: &str) -> String {
        let mut out = input.to_string();
        for (name, re) in &self.rules {
            let marker = format!("[REDACTED:{name}]");
            out = re.replace_all(&out, marker.as_str()).into_owned();
        }
        out
    }
}

impl Default for Redactor {
    fn default() -> Self {
        Self::new(&default_patterns())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_openai_key() {
        let r = Redactor::default();
        let out = r.redact("token=sk-proj-EXAMPLE_FIXTURE_NOT_A_REAL_KEY_aaaaaaaaaa");
        assert!(!out.contains("sk-proj-EXAMPLE"));
        assert!(out.contains("[REDACTED:openai_key]"));
    }

    #[test]
    fn passthrough_normal_text() {
        let r = Redactor::default();
        assert_eq!(r.redact("hello world"), "hello world");
    }

    #[test]
    fn redacts_anthropic_key() {
        let r = Redactor::default();
        let out = r.redact("ANTHROPIC_API_KEY=sk-ant-api03-AAAAAAAAAAAAAAAAAAAA");
        assert!(out.contains("[REDACTED:anthropic_key]"));
    }

    #[test]
    fn redacts_jwt() {
        let r = Redactor::default();
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.SflKxwRJSMeKKF2QT4f";
        let out = r.redact(&format!("auth: {jwt}"));
        assert!(out.contains("[REDACTED:jwt]"));
    }

    #[test]
    fn redacts_pem_multiline() {
        let r = Redactor::default();
        let pem = "-----BEGIN RSA PRIVATE KEY-----\nMIIE...\nABC=\n-----END RSA PRIVATE KEY-----";
        let out = r.redact(pem);
        assert!(out.contains("[REDACTED:pem_private_key]"));
        assert!(!out.contains("MIIE"));
    }

    #[test]
    fn redacts_dotenv_secret() {
        let r = Redactor::default();
        let out = r.redact("DATABASE_PASSWORD=super-s3cret-value\nUNRELATED=ok");
        assert!(out.contains("[REDACTED:dotenv_secret]"));
        assert!(out.contains("UNRELATED=ok"));
    }

    #[test]
    fn redacts_gcp_api_key() {
        let r = Redactor::default();
        let out = r.redact("key: AIzaSyA-1234567890abcdefghijklmnopqrstuvw");
        assert!(out.contains("[REDACTED:gcp_api_key]"));
    }

    #[test]
    fn idempotent_on_already_redacted() {
        let r = Redactor::default();
        let once = r.redact("sk-proj-EXAMPLE_FIXTURE_NOT_A_REAL_KEY_aaaaaaaaaa");
        let twice = r.redact(&once);
        assert_eq!(once, twice);
    }
}
