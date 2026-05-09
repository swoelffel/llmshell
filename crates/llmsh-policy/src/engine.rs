use crate::context::{CheckedToolCall, PolicyContext};
use crate::types::{PolicyAction, PolicyDecision, PolicyFlag, RiskLevel};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub trait PolicyEngine: Send + Sync {
    fn evaluate(&self, call: &CheckedToolCall, ctx: &PolicyContext) -> PolicyDecision;
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RiskAction {
    Allow,
    Confirm,
    ConfirmStrong,
    Deny,
}

#[derive(Debug, Clone)]
pub struct DefaultPolicyConfig {
    pub risk_actions: HashMap<RiskLevel, RiskAction>,
}

impl Default for DefaultPolicyConfig {
    fn default() -> Self {
        let mut m = HashMap::new();
        m.insert(RiskLevel::ReadOnly, RiskAction::Allow);
        m.insert(RiskLevel::LowRisk, RiskAction::Allow);
        m.insert(RiskLevel::Write, RiskAction::Confirm);
        m.insert(RiskLevel::Destructive, RiskAction::ConfirmStrong);
        m.insert(RiskLevel::Network, RiskAction::Confirm);
        m.insert(RiskLevel::Privileged, RiskAction::ConfirmStrong);
        m.insert(RiskLevel::Unknown, RiskAction::Confirm);
        Self { risk_actions: m }
    }
}

pub struct DefaultPolicyEngine {
    pub config: DefaultPolicyConfig,
}

impl DefaultPolicyEngine {
    pub fn new(config: DefaultPolicyConfig) -> Self {
        Self { config }
    }
}

impl PolicyEngine for DefaultPolicyEngine {
    fn evaluate(&self, call: &CheckedToolCall, _ctx: &PolicyContext) -> PolicyDecision {
        let mut reasons = Vec::new();
        let mut effective = call.declared_risk;
        let mut flags = call.flags.clone();

        let has_sensitive = call.resolved_paths.iter().any(|p| p.matches_sensitive)
            || flags.contains(&PolicyFlag::SensitivePath);
        if has_sensitive {
            if !flags.contains(&PolicyFlag::SensitivePath) {
                flags.push(PolicyFlag::SensitivePath);
            }
            reasons.push("path matches sensitive_paths pattern".into());
        }

        if flags.contains(&PolicyFlag::LargeBlastRadius) {
            effective = RiskLevel::Destructive;
            reasons.push("large blast radius detected".into());
        }
        if flags.contains(&PolicyFlag::UsesPrivilegeEscalation) {
            effective = RiskLevel::Privileged;
            reasons.push("privilege escalation detected".into());
        }

        let mut action = match self
            .config
            .risk_actions
            .get(&effective)
            .copied()
            .unwrap_or(RiskAction::Confirm)
        {
            RiskAction::Allow => PolicyAction::Allow,
            RiskAction::Confirm => PolicyAction::RequireConfirmation {
                strong: false,
                phrase: None,
            },
            RiskAction::ConfirmStrong => PolicyAction::RequireConfirmation {
                strong: true,
                phrase: Some(crate::phrase::generate_phrase(call)),
            },
            RiskAction::Deny => PolicyAction::Deny,
        };

        // Sensitive paths force at least ConfirmStrong (unless already Deny).
        if has_sensitive {
            action = match action {
                PolicyAction::Allow => PolicyAction::RequireConfirmation {
                    strong: true,
                    phrase: Some(crate::phrase::generate_phrase(call)),
                },
                PolicyAction::RequireConfirmation {
                    strong: false,
                    phrase: _,
                } => PolicyAction::RequireConfirmation {
                    strong: true,
                    phrase: Some(crate::phrase::generate_phrase(call)),
                },
                other => other,
            };
        }

        PolicyDecision {
            effective_risk: effective,
            action,
            flags,
            reasons,
        }
    }
}

#[cfg(test)]
mod eng_tests {
    use super::*;
    use crate::context::ResolvedPath;
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::{Arc, RwLock};

    fn ctx() -> PolicyContext {
        PolicyContext {
            cwd: Arc::new(RwLock::new(PathBuf::from("/tmp"))),
            workspace_root: PathBuf::from("/tmp"),
            allowed_roots: vec![PathBuf::from("/tmp")],
            sensitive_path_patterns: vec![],
        }
    }

    #[test]
    fn read_only_allowed() {
        let eng = DefaultPolicyEngine::new(DefaultPolicyConfig::default());
        let call = CheckedToolCall {
            id: "1".into(),
            tool_name: "read_file".into(),
            args: json!({"path":"x"}),
            declared_risk: RiskLevel::ReadOnly,
            resolved_paths: vec![],
            flags: vec![],
        };
        assert!(matches!(
            eng.evaluate(&call, &ctx()).action,
            PolicyAction::Allow
        ));
    }

    #[test]
    fn sensitive_path_requires_strong_confirm() {
        let eng = DefaultPolicyEngine::new(DefaultPolicyConfig::default());
        let call = CheckedToolCall {
            id: "1".into(),
            tool_name: "read_file".into(),
            args: json!({"path":"~/.ssh/id_rsa"}),
            declared_risk: RiskLevel::ReadOnly,
            resolved_paths: vec![ResolvedPath {
                original: "~/.ssh/id_rsa".into(),
                canonical: PathBuf::from("/home/u/.ssh/id_rsa"),
                matches_sensitive: true,
            }],
            flags: vec![],
        };
        let d = eng.evaluate(&call, &ctx());
        match d.action {
            PolicyAction::RequireConfirmation {
                strong: true,
                phrase: Some(_),
            } => {}
            other => panic!("expected strong confirm, got {:?}", other),
        }
        assert!(d.flags.contains(&PolicyFlag::SensitivePath));
    }

    #[test]
    fn destructive_requires_strong_confirm_with_phrase() {
        let eng = DefaultPolicyEngine::new(DefaultPolicyConfig::default());
        let call = CheckedToolCall {
            id: "1".into(),
            tool_name: "run_process".into(),
            args: json!({"program":"rm","args":["-rf","./x"]}),
            declared_risk: RiskLevel::Unknown,
            resolved_paths: vec![],
            flags: vec![PolicyFlag::LargeBlastRadius, PolicyFlag::UsesShell],
        };
        let d = eng.evaluate(&call, &ctx());
        match d.action {
            PolicyAction::RequireConfirmation {
                strong: true,
                phrase: Some(p),
            } => {
                assert!(p.contains("rm"));
            }
            _ => panic!("expected strong confirm"),
        }
    }

    #[test]
    fn outside_workspace_no_longer_denies() {
        // Previously a path outside workspace was Deny. Now there is no
        // workspace boundary — the path doesn't matter unless it is
        // sensitive.
        let eng = DefaultPolicyEngine::new(DefaultPolicyConfig::default());
        let call = CheckedToolCall {
            id: "1".into(),
            tool_name: "read_file".into(),
            args: json!({"path":"/etc/hostname"}),
            declared_risk: RiskLevel::ReadOnly,
            resolved_paths: vec![ResolvedPath {
                original: "/etc/hostname".into(),
                canonical: PathBuf::from("/etc/hostname"),
                matches_sensitive: false,
            }],
            flags: vec![],
        };
        assert!(matches!(
            eng.evaluate(&call, &ctx()).action,
            PolicyAction::Allow
        ));
    }
}
