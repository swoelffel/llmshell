use regex::Regex;

pub struct Redactor {
    patterns: Vec<(String, Regex)>,
}

impl Redactor {
    pub fn default_audit() -> Self {
        let raw = [
            ("openai_key", r"sk-[A-Za-z0-9]{20,}"),
            ("anthropic_key", r"sk-ant-[A-Za-z0-9-]{20,}"),
            ("github_token", r"gh[pousr]_[A-Za-z0-9]{20,}"),
            ("aws_access_key", r"AKIA[0-9A-Z]{16}"),
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
            .map(|(n, p)| (n.to_string(), Regex::new(p).expect("regex")))
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
