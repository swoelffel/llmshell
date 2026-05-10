use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum RiskLevel {
    ReadOnly,
    LowRisk,
    Write,
    Destructive,
    Network,
    Privileged,
    Unknown,
}

impl RiskLevel {
    /// Numeric severity. Higher = riskier. Used to compare model claims to
    /// classifier output (upgrade-only).
    pub fn severity(self) -> u8 {
        match self {
            RiskLevel::ReadOnly => 0,
            RiskLevel::LowRisk => 1,
            RiskLevel::Network => 2,
            RiskLevel::Write => 3,
            RiskLevel::Unknown => 4,
            RiskLevel::Destructive => 5,
            RiskLevel::Privileged => 6,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum PolicyFlag {
    SensitivePath,
    SecretLikeContent,
    OutsideWorkspace,
    LargeBlastRadius,
    UsesShell,
    UsesPrivilegeEscalation,
    /// Set by the pipeline when the deterministic classifier matched a
    /// read-only `run_process` invocation. Audit-visible.
    KnownReadOnlyCommand,
    /// LLM declared a `claimed_risk` higher than the classifier's verdict.
    /// The higher value was taken; this flag records that the model contributed.
    ModelClaimedRisk,
    /// LLM declared a `claimed_risk` lower than the classifier's verdict.
    /// The classifier value was kept; this flag records the disagreement
    /// for offline review.
    ModelDisagreesOnRisk,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum PolicyAction {
    Allow,
    RequireConfirmation {
        strong: bool,
        phrase: Option<String>,
    },
    Deny,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyDecision {
    pub effective_risk: RiskLevel,
    pub action: PolicyAction,
    pub flags: Vec<PolicyFlag>,
    pub reasons: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn risk_level_serializes_snake_case() {
        assert_eq!(
            serde_json::to_string(&RiskLevel::ReadOnly).unwrap(),
            "\"read_only\""
        );
        assert_eq!(
            serde_json::to_string(&RiskLevel::Destructive).unwrap(),
            "\"destructive\""
        );
    }

    #[test]
    fn policy_action_tagged() {
        let a = PolicyAction::RequireConfirmation {
            strong: true,
            phrase: Some("delete 3 files".into()),
        };
        let s = serde_json::to_string(&a).unwrap();
        assert!(s.contains("\"kind\":\"require_confirmation\""));
        assert!(s.contains("\"strong\":true"));
    }
}
