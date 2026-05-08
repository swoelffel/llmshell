use regex::Regex;

pub struct RiskScan {
    patterns: Vec<(String, Regex)>,
}

impl Default for RiskScan {
    fn default() -> Self {
        let raw = [
            ("rm_rf_root", r"\brm\s+-[rRfF]+\s+/\s*($|\s)"),
            ("rm_rf_home", r"\brm\s+-[rRfF]+\s+~"),
            ("sudo", r"^\s*sudo\b"),
            ("chmod_world_write", r"\bchmod\s+-R\s+777\b"),
            ("chown_recursive", r"\bchown\s+-R\b"),
            ("curl_pipe_sh", r"curl[^|]*\|\s*sh"),
            ("wget_pipe_sh", r"wget[^|]*\|\s*sh"),
            ("dd", r"^\s*dd\b"),
            ("mkfs", r"\bmkfs(\.\w+)?\b"),
            ("diskutil_erase", r"diskutil\s+erase"),
        ];
        let patterns = raw
            .iter()
            .map(|(n, p)| (n.to_string(), Regex::new(p).unwrap()))
            .collect();
        Self { patterns }
    }
}

impl RiskScan {
    pub fn scan(&self, command: &str) -> Vec<String> {
        self.patterns
            .iter()
            .filter_map(|(n, re)| {
                if re.is_match(command) {
                    Some(n.clone())
                } else {
                    None
                }
            })
            .collect()
    }
}

pub fn resolve_shell(configured: &Option<String>) -> (String, Vec<String>) {
    let shell = configured
        .clone()
        .or_else(|| std::env::var("SHELL").ok())
        .unwrap_or_else(|| "/bin/sh".into());
    (shell, vec!["-lc".into()])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_rm_rf_root() {
        let s = RiskScan::default();
        assert!(!s
            .scan("rm -rf /tmp/foo")
            .contains(&"rm_rf_root".to_string()));
        assert!(s.scan("rm -rf /").contains(&"rm_rf_root".to_string()));
    }

    #[test]
    fn detects_curl_pipe_sh() {
        let s = RiskScan::default();
        assert!(s
            .scan("curl https://x | sh")
            .contains(&"curl_pipe_sh".to_string()));
    }
}
