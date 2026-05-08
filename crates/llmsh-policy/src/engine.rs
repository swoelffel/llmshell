use crate::context::{CheckedToolCall, PolicyContext};
use crate::types::{PolicyDecision, RiskLevel};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub trait PolicyEngine: Send + Sync {
    fn evaluate(&self, call: &CheckedToolCall, ctx: &PolicyContext) -> PolicyDecision;
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RiskAction { Allow, Confirm, ConfirmStrong, Deny }

#[derive(Debug, Clone)]
pub struct DefaultPolicyConfig {
    pub risk_actions: HashMap<RiskLevel, RiskAction>,
    pub sensitive_paths_action: RiskAction,
    pub allow_outside_workspace: bool,
}

impl Default for DefaultPolicyConfig {
    fn default() -> Self {
        let mut m = HashMap::new();
        m.insert(RiskLevel::ReadOnly,    RiskAction::Allow);
        m.insert(RiskLevel::LowRisk,     RiskAction::Allow);
        m.insert(RiskLevel::Write,       RiskAction::Confirm);
        m.insert(RiskLevel::Destructive, RiskAction::ConfirmStrong);
        m.insert(RiskLevel::Network,     RiskAction::Confirm);
        m.insert(RiskLevel::Privileged,  RiskAction::Deny);
        m.insert(RiskLevel::Unknown,     RiskAction::Confirm);
        Self {
            risk_actions: m,
            sensitive_paths_action: RiskAction::Deny,
            allow_outside_workspace: false,
        }
    }
}

pub struct DefaultPolicyEngine {
    pub config: DefaultPolicyConfig,
}

impl DefaultPolicyEngine {
    pub fn new(config: DefaultPolicyConfig) -> Self { Self { config } }
}
