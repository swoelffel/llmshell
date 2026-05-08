use llmsh_llm::types::{LlmResponse, ToolCall};
use llmsh_policy::context::CheckedToolCall;
use llmsh_policy::types::PolicyDecision;

pub struct ModelPlan {
    pub message: Option<String>,
    pub tool_calls: Vec<ToolCall>,
}

impl From<LlmResponse> for ModelPlan {
    fn from(r: LlmResponse) -> Self {
        Self {
            message: r.message,
            tool_calls: r.tool_calls,
        }
    }
}

pub struct CheckedStep {
    pub call: CheckedToolCall,
    pub decision: PolicyDecision,
}

pub struct CheckedPlan {
    pub plan_id: String,
    pub steps: Vec<CheckedStep>,
}

impl CheckedPlan {
    pub fn requires_confirmation(&self) -> bool {
        self.steps.iter().any(|s| {
            matches!(
                s.decision.action,
                llmsh_policy::types::PolicyAction::RequireConfirmation { .. }
            )
        })
    }
    pub fn has_deny(&self) -> bool {
        self.steps.iter().any(|s| {
            matches!(s.decision.action, llmsh_policy::types::PolicyAction::Deny)
        })
    }
}
