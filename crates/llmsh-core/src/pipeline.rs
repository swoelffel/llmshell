use crate::plan::{CheckedPlan, CheckedStep, ModelPlan};
use llmsh_llm::types::ToolCall;
use llmsh_policy::context::PolicyContext;
use llmsh_policy::engine::PolicyEngine;
use llmsh_policy::types::{PolicyAction, PolicyDecision, RiskLevel};
use llmsh_tools::enrich::{enrich, EnrichmentInput};
use llmsh_tools::registry::ToolRegistry;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(thiserror::Error, Debug)]
pub enum PipelineError {
    #[error("unknown tool: {0}")]
    UnknownTool(String),
    #[error("schema validation failed: {0}")]
    Schema(String),
}

pub struct Pipeline {
    pub registry: Arc<ToolRegistry>,
    pub policy: Arc<dyn PolicyEngine>,
    pub home: Option<PathBuf>,
}

pub struct CheckOutcome {
    pub plan: CheckedPlan,
    pub schema_errors: Vec<(String, String)>, // (tool_call_id, message)
}

impl Pipeline {
    pub fn check(
        &self,
        model: ModelPlan,
        ctx: &PolicyContext,
        sensitive_patterns: &[String],
    ) -> CheckOutcome {
        let mut steps = Vec::new();
        let mut schema_errors = Vec::new();
        for tc in model.tool_calls {
            let step = match self.check_one(&tc, ctx, sensitive_patterns) {
                Ok(s) => s,
                Err(PipelineError::UnknownTool(name)) => CheckedStep {
                    call: llmsh_policy::context::CheckedToolCall {
                        id: tc.id.clone(),
                        tool_name: name.clone(),
                        args: tc.args.clone(),
                        declared_risk: RiskLevel::Unknown,
                        resolved_paths: vec![],
                        flags: vec![],
                    },
                    decision: PolicyDecision {
                        effective_risk: RiskLevel::Unknown,
                        action: PolicyAction::Deny,
                        flags: vec![],
                        reasons: vec![format!("tool not in registry: {}", name)],
                    },
                },
                Err(PipelineError::Schema(msg)) => {
                    schema_errors.push((tc.id.clone(), msg.clone()));
                    CheckedStep {
                        call: llmsh_policy::context::CheckedToolCall {
                            id: tc.id.clone(),
                            tool_name: tc.name.clone(),
                            args: tc.args.clone(),
                            declared_risk: RiskLevel::Unknown,
                            resolved_paths: vec![],
                            flags: vec![],
                        },
                        decision: PolicyDecision {
                            effective_risk: RiskLevel::Unknown,
                            action: PolicyAction::Deny,
                            flags: vec![],
                            reasons: vec![msg],
                        },
                    }
                }
            };
            steps.push(step);
        }
        let plan_id = format!("plan-{}", uuid_short());
        CheckOutcome {
            plan: CheckedPlan { plan_id, steps },
            schema_errors,
        }
    }

    fn check_one(
        &self,
        tc: &ToolCall,
        ctx: &PolicyContext,
        sensitive: &[String],
    ) -> Result<CheckedStep, PipelineError> {
        let tool = self
            .registry
            .get(&tc.name)
            .ok_or_else(|| PipelineError::UnknownTool(tc.name.clone()))?;
        // Minimal schema validation: required fields per top-level schema "required" array.
        validate_schema(&tool.input_schema(), &tc.args).map_err(PipelineError::Schema)?;
        let enriched = enrich(
            tc,
            tool.declared_risk(),
            EnrichmentInput {
                cwd: &ctx.cwd,
                workspace_root: &ctx.workspace_root,
                home: self.home.as_deref(),
                sensitive_patterns: sensitive,
            },
        );
        let decision = self.policy.evaluate(&enriched, ctx);
        Ok(CheckedStep {
            call: enriched,
            decision,
        })
    }
}

fn uuid_short() -> String {
    uuid::Uuid::new_v4().to_string()[..8].to_string()
}

fn validate_schema(schema: &serde_json::Value, args: &serde_json::Value) -> Result<(), String> {
    let required = schema
        .get("required")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let obj = args.as_object().ok_or("args must be an object")?;
    for r in required {
        let name = r.as_str().ok_or("invalid required entry")?;
        if !obj.contains_key(name) {
            return Err(format!("missing required field {}", name));
        }
    }
    Ok(())
}
