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

/// Why a `run_process` invocation could not be proven read-only. Mirrors
/// `RiskLevel::Unknown` with a structured explanation that the
/// confirmation gate and audit log can surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClassificationReason {
    /// Default before any classification was attempted (e.g. auto-classify
    /// disabled, or non-`run_process` tool).
    UnclassifiedDefault,
    /// Program name contains `/` — only PATH lookups are classified.
    AbsoluteOrRelativePath,
    /// Program isn't on the read-only allowlist.
    ProgramNotAllowlisted,
    /// Tool-specific argument disqualified the invocation.
    UnsafeArgument,
    /// Shell payload contains command substitution or process substitution.
    CommandSubstitution,
    /// Shell payload uses variable expansion.
    VariableExpansion,
    /// Shell payload uses a glob we cannot expand statically.
    GlobNotResolved,
    /// Shell payload uses `;` sequence or `&` background.
    SequenceOrBackground,
    /// Output redirection target other than `/dev/null` or `/tmp/<simple>`.
    UnsafeRedirectionTarget,
    /// At least one pipeline segment is not classified as read-only.
    UnsafePipelineSegment,
    /// Shell payload nests another shell wrapper.
    NestedShellWrapping,
    /// Shell payload could not be tokenized.
    UnparsableShellPayload,
    /// Outer call is a shell but argv shape isn't `-c PAYLOAD …`.
    NotShellDashCForm,
}

impl ClassificationReason {
    /// Short human label used in the confirmation prompt.
    pub fn label(self) -> &'static str {
        match self {
            ClassificationReason::UnclassifiedDefault => "pas de classification statique",
            ClassificationReason::AbsoluteOrRelativePath => "chemin absolu/relatif rejeté",
            ClassificationReason::ProgramNotAllowlisted => "programme hors allowlist read-only",
            ClassificationReason::UnsafeArgument => "argument non read-only",
            ClassificationReason::CommandSubstitution => "substitution de commande",
            ClassificationReason::VariableExpansion => "expansion de variable",
            ClassificationReason::GlobNotResolved => "glob non résolu",
            ClassificationReason::SequenceOrBackground => "séquence ; ou &",
            ClassificationReason::UnsafeRedirectionTarget => "redirection vers cible non sûre",
            ClassificationReason::UnsafePipelineSegment => "segment de pipeline non read-only",
            ClassificationReason::NestedShellWrapping => "shell imbriqué",
            ClassificationReason::UnparsableShellPayload => "payload shell non parseable",
            ClassificationReason::NotShellDashCForm => "shell sans -c PAYLOAD",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum PolicyAction {
    Allow,
    RequireConfirmation {
        strong: bool,
        /// Single-keystroke default-yes prompt. Used when the classifier
        /// could not prove read-only (`effective_risk == Unknown`) but the
        /// LLM declared a low-severity `claimed_risk`. The model never
        /// has authority over execution: a confirmation is still required,
        /// only the prompt is lighter.
        #[serde(default)]
        light: bool,
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
    /// Structured reason explaining why the deterministic classifier did
    /// not return `ReadOnly`. `None` for tools that aren't subject to the
    /// classifier, or when the call *was* classified as read-only.
    #[serde(default)]
    pub classification_reason: Option<ClassificationReason>,
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
            light: false,
            phrase: Some("delete 3 files".into()),
        };
        let s = serde_json::to_string(&a).unwrap();
        assert!(s.contains("\"kind\":\"require_confirmation\""));
        assert!(s.contains("\"strong\":true"));
    }
}
