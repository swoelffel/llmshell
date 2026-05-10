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
        // Privilege-escalation flag: post-deshell so `bash -c "sudo …"` is caught.
        if enriched.tool_name == "run_process"
            && detects_privilege_escalation(&enriched.args)
            && !enriched
                .flags
                .contains(&llmsh_policy::types::PolicyFlag::UsesPrivilegeEscalation)
        {
            enriched
                .flags
                .push(llmsh_policy::types::PolicyFlag::UsesPrivilegeEscalation);
        }
        // Apply LLM-claimed risk as upgrade-only. The model can RAISE the
        // risk level above the deterministic verdict but never lower it.
        if enriched.tool_name == "run_process" {
            if let Some(claimed_str) = enriched.args.get("claimed_risk").and_then(|v| v.as_str()) {
                if let Some(claimed) = llmsh_policy::safe_commands::parse_claimed_risk(claimed_str)
                {
                    use llmsh_policy::types::PolicyFlag;
                    if claimed.severity() > enriched.declared_risk.severity() {
                        enriched.declared_risk = claimed;
                        if !enriched.flags.contains(&PolicyFlag::ModelClaimedRisk) {
                            enriched.flags.push(PolicyFlag::ModelClaimedRisk);
                        }
                    } else if claimed.severity() < enriched.declared_risk.severity()
                        && !enriched.flags.contains(&PolicyFlag::ModelDisagreesOnRisk)
                    {
                        enriched.flags.push(PolicyFlag::ModelDisagreesOnRisk);
                    }
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

/// Inspect the outer (program, args) pair AND, if it is a `bash -c "…"` form
/// that the safe_commands deshell would accept, the inner program too.
/// Returns `true` iff either layer is `sudo`/`doas`/`su`.
fn detects_privilege_escalation(args: &serde_json::Value) -> bool {
    let Some(program) = args.get("program").and_then(|v| v.as_str()) else {
        return false;
    };
    if matches!(program, "sudo" | "doas" | "su") {
        return true;
    }
    const SHELLS: &[&str] = &[
        "bash", "sh", "zsh", "fish", "dash", "ksh", "csh", "tcsh", "ash",
    ];
    if !SHELLS.contains(&program) {
        return false;
    }
    let arg_arr = match args.get("args").and_then(|v| v.as_array()) {
        Some(a) => a,
        None => return false,
    };
    if arg_arr.len() != 2 {
        return false;
    }
    if arg_arr[0].as_str() != Some("-c") {
        return false;
    }
    let payload = match arg_arr[1].as_str() {
        Some(p) => p,
        None => return false,
    };
    let tokens = match shlex::split(payload) {
        Some(t) => t,
        None => return false,
    };
    matches!(
        tokens.first().map(String::as_str),
        Some("sudo") | Some("doas") | Some("su")
    )
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
            model_plan(serde_json::json!({
                "program":"ls","args":["-la"],
                "intent":"list","claimed_risk":"read_only"
            })),
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
            model_plan(serde_json::json!({
                "program":"ls",
                "intent":"list","claimed_risk":"read_only"
            })),
            &ctx(),
            &[],
        );
        assert_eq!(out.plan.steps[0].call.declared_risk, RiskLevel::Unknown);
    }

    #[test]
    fn claimed_destructive_upgrades_classifier_read_only() {
        let p = make_pipeline(true);
        let out = p.check(
            model_plan(serde_json::json!({
                "program":"ls",
                "intent":"list before deleting",
                "claimed_risk":"destructive"
            })),
            &ctx(),
            &[],
        );
        let step = &out.plan.steps[0];
        assert_eq!(step.call.declared_risk, RiskLevel::Destructive);
        assert!(step.call.flags.contains(&PolicyFlag::ModelClaimedRisk));
    }

    #[test]
    fn claimed_read_only_does_not_downgrade_unknown() {
        let p = make_pipeline(true);
        let out = p.check(
            model_plan(serde_json::json!({
                "program":"rm","args":["-rf","/tmp/x"],
                "intent":"clean build artefacts",
                "claimed_risk":"read_only"
            })),
            &ctx(),
            &[],
        );
        let step = &out.plan.steps[0];
        assert_eq!(step.call.declared_risk, RiskLevel::Unknown);
        assert!(step.call.flags.contains(&PolicyFlag::ModelDisagreesOnRisk));
    }

    #[test]
    fn privesc_flag_set_for_direct_sudo() {
        let p = make_pipeline(true);
        let out = p.check(
            model_plan(serde_json::json!({
                "program":"sudo","args":["softwareupdate","--install","--all"],
                "intent":"update","claimed_risk":"write"
            })),
            &ctx(),
            &[],
        );
        let step = &out.plan.steps[0];
        assert!(step
            .call
            .flags
            .contains(&PolicyFlag::UsesPrivilegeEscalation));
        assert_eq!(step.decision.effective_risk, RiskLevel::Privileged);
    }

    #[test]
    fn privesc_flag_set_through_bash_wrapper() {
        let p = make_pipeline(true);
        let out = p.check(
            model_plan(serde_json::json!({
                "program":"bash","args":["-c","sudo softwareupdate --install --all"],
                "intent":"update","claimed_risk":"write"
            })),
            &ctx(),
            &[],
        );
        let step = &out.plan.steps[0];
        assert!(
            step.call
                .flags
                .contains(&PolicyFlag::UsesPrivilegeEscalation),
            "flag must follow through bash -c wrapper"
        );
        assert_eq!(step.decision.effective_risk, RiskLevel::Privileged);
    }

    #[test]
    fn privesc_flag_not_set_for_plain_ls() {
        let p = make_pipeline(true);
        let out = p.check(
            model_plan(serde_json::json!({
                "program":"ls","args":["-la"],
                "intent":"list","claimed_risk":"read_only"
            })),
            &ctx(),
            &[],
        );
        assert!(!out.plan.steps[0]
            .call
            .flags
            .contains(&PolicyFlag::UsesPrivilegeEscalation));
    }

    #[test]
    fn rm_stays_unknown() {
        let p = make_pipeline(true);
        let out = p.check(
            model_plan(serde_json::json!({
                "program":"rm","args":["-rf","/tmp/x"],
                "intent":"clean","claimed_risk":"unknown"
            })),
            &ctx(),
            &[],
        );
        assert_eq!(out.plan.steps[0].call.declared_risk, RiskLevel::Unknown);
    }
}
