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
    pub auto_classify_run_process: bool,
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
        let cwd_snap = ctx.cwd_snapshot();
        let mut enriched = enrich(
            tc,
            tool.declared_risk(),
            EnrichmentInput {
                cwd: &cwd_snap,
                home: self.home.as_deref(),
                sensitive_patterns: sensitive,
            },
        );
        if self.auto_classify_run_process && enriched.tool_name == "run_process" {
            if let Some(downgraded) = classify_run_process_args(&enriched.args) {
                enriched.declared_risk = downgraded;
                if !enriched
                    .flags
                    .contains(&llmsh_policy::types::PolicyFlag::KnownReadOnlyCommand)
                {
                    enriched
                        .flags
                        .push(llmsh_policy::types::PolicyFlag::KnownReadOnlyCommand);
                }
            }
        }
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

fn classify_run_process_args(args: &serde_json::Value) -> Option<RiskLevel> {
    let program = args.get("program").and_then(|v| v.as_str())?;
    let arg_list: Vec<String> = args
        .get("args")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    llmsh_policy::safe_commands::is_read_only_invocation(program, &arg_list)
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

#[cfg(test)]
mod classifier_tests {
    use super::*;
    use llmsh_llm::types::ToolCall;
    use llmsh_policy::engine::{DefaultPolicyConfig, DefaultPolicyEngine};
    use llmsh_policy::types::PolicyFlag;
    use llmsh_tools::registry::ToolRegistry;
    use llmsh_tools::run_process::RunProcess;
    use std::path::PathBuf;
    use std::sync::{Arc, RwLock};

    fn make_pipeline(auto: bool) -> Pipeline {
        let mut reg = ToolRegistry::new();
        reg.register(Arc::new(RunProcess));
        Pipeline {
            registry: Arc::new(reg),
            policy: Arc::new(DefaultPolicyEngine::new(DefaultPolicyConfig::default())),
            home: None,
            auto_classify_run_process: auto,
        }
    }

    fn ctx() -> PolicyContext {
        PolicyContext {
            cwd: Arc::new(RwLock::new(PathBuf::from("/tmp"))),
            workspace_root: PathBuf::from("/tmp"),
            allowed_roots: vec![],
            sensitive_path_patterns: vec![],
        }
    }

    fn model_plan(args: serde_json::Value) -> crate::plan::ModelPlan {
        crate::plan::ModelPlan {
            message: None,
            tool_calls: vec![ToolCall {
                id: "c1".into(),
                name: "run_process".into(),
                args,
            }],
        }
    }

    #[test]
    fn ls_downgraded_to_read_only_when_enabled() {
        let p = make_pipeline(true);
        let out = p.check(
            model_plan(serde_json::json!({"program":"ls","args":["-la"]})),
            &ctx(),
            &[],
        );
        let step = &out.plan.steps[0];
        assert_eq!(step.call.declared_risk, RiskLevel::ReadOnly);
        assert!(step.call.flags.contains(&PolicyFlag::KnownReadOnlyCommand));
        assert!(matches!(step.decision.action, PolicyAction::Allow));
    }

    #[test]
    fn ls_stays_unknown_when_disabled() {
        let p = make_pipeline(false);
        let out = p.check(
            model_plan(serde_json::json!({"program":"ls"})),
            &ctx(),
            &[],
        );
        assert_eq!(out.plan.steps[0].call.declared_risk, RiskLevel::Unknown);
    }

    #[test]
    fn rm_stays_unknown() {
        let p = make_pipeline(true);
        let out = p.check(
            model_plan(serde_json::json!({"program":"rm","args":["-rf","/tmp/x"]})),
            &ctx(),
            &[],
        );
        assert_eq!(out.plan.steps[0].call.declared_risk, RiskLevel::Unknown);
    }
}
