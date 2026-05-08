use crate::context::{ContextBuilder, SystemPromptSource};
use crate::executor::ToolExecutor;
use crate::memory::{ActionKind, Memory, RecentAction};
use crate::pipeline::Pipeline;
use crate::plan::ModelPlan;
use crate::session_stats::SessionStats;
use llmsh_audit::digest::canonical_json_digest;
use llmsh_audit::event::{now_iso, AuditEvent};
use llmsh_audit::redact::Redactor;
use llmsh_audit::writer::AuditWriter;
use llmsh_llm::provider::LlmProvider;
use llmsh_llm::types::{FinishReason, LlmRequest, ToolPolicyHint};
use llmsh_policy::context::PolicyContext;
use std::sync::Arc;
use std::time::Instant;

pub struct AgentBounds {
    pub max_iterations: u32,
    pub max_tool_calls_per_iteration: u32,
    pub max_schema_repair_attempts: u32,
}

pub struct AgentDeps {
    pub provider: Arc<dyn LlmProvider>,
    pub pipeline: Pipeline,
    pub executor: ToolExecutor,
    pub gate: Arc<dyn crate::confirm::ConfirmationGate>,
    pub audit: std::sync::Mutex<AuditWriter>,
    pub redactor: Redactor,
    pub bounds: AgentBounds,
    pub policy_ctx: PolicyContext,
    pub sensitive_patterns: Vec<String>,
    pub model_label: std::sync::Arc<std::sync::RwLock<String>>,
    pub system_prompt: Arc<dyn SystemPromptSource>,
    pub memory: Arc<Memory>,
    /// 0 = silent, 1 = tier-1, 2 = tier-1 + tier-2.
    pub verbose: u8,
    /// Live session stats; shared with the status-line prompt.
    pub stats: Arc<std::sync::RwLock<SessionStats>>,
}

pub struct AgentLoop {
    pub deps: Arc<AgentDeps>,
    pub builder: ContextBuilder,
}

pub struct LoopResult {
    pub assistant_text: Option<String>,
    pub stopped_reason: String,
}

impl AgentLoop {
    pub async fn run(&mut self, user_input: &str) -> anyhow::Result<LoopResult> {
        let dep = self.deps.clone();

        let user_input_red = dep.redactor.redact(user_input).0;
        if let Err(e) = dep.memory.append_action(&RecentAction {
            ts: now_iso(),
            kind: ActionKind::UserInput,
            summary: user_input_red,
            detail_json: None,
        }) {
            tracing::warn!("memory append_action(user_input) failed: {}", e);
        }

        self.builder.append_user(user_input);

        let mut iter = 0u32;
        let mut schema_attempts = 0u32;
        loop {
            iter += 1;
            if iter > dep.bounds.max_iterations {
                let _ = dep.audit.lock().unwrap().write(&AuditEvent::Error {
                    ts: now_iso(),
                    code: "max_iterations".into(),
                    message: "agent loop exceeded max iterations".into(),
                    context_redacted: None,
                });
                return Ok(LoopResult {
                    assistant_text: None,
                    stopped_reason: "max_iterations".into(),
                });
            }

            let req = LlmRequest {
                system: Some(dep.system_prompt.current()),
                messages: self.builder.messages.clone(),
                tools: dep.pipeline.registry.specs(),
                tool_policy: ToolPolicyHint::PreferTools,
            };
            let messages_digest = canonical_json_digest(&serde_json::to_value(&req.messages)?);
            let model_snap = dep
                .model_label
                .read()
                .map(|g| g.clone())
                .unwrap_or_else(|_| "unknown".into());
            let _ = dep.audit.lock().unwrap().write(&AuditEvent::LlmRequest {
                ts: now_iso(),
                model: model_snap.clone(),
                messages_digest,
                tool_count: req.tools.len(),
                prompt_token_estimate: None,
                context_bytes: serde_json::to_string(&req.messages)?.len(),
                redaction_applied: true,
                redaction_hit_count: 0,
            });

            let started = Instant::now();
            let resp = dep.provider.complete(req).await?;
            let latency = started.elapsed();
            {
                let model_now = dep
                    .model_label
                    .read()
                    .map(|g| g.clone())
                    .unwrap_or_else(|_| "unknown".into());
                if let Ok(mut s) = dep.stats.write() {
                    s.record_turn(&model_now, resp.usage.as_ref(), resp.finish_reason, latency);
                }
            }
            let tool_calls_digest = if resp.tool_calls.is_empty() {
                None
            } else {
                Some(canonical_json_digest(&serde_json::to_value(
                    &resp.tool_calls,
                )?))
            };
            let msg_red = resp
                .message
                .as_deref()
                .map(|m| dep.redactor.redact(m).0)
                .unwrap_or_default();
            let _ = dep.audit.lock().unwrap().write(&AuditEvent::LlmResponse {
                ts: now_iso(),
                model: model_snap.clone(),
                finish_reason: format!("{:?}", resp.finish_reason).to_lowercase(),
                message_redacted: resp.message.as_ref().map(|_| msg_red),
                tool_call_count: resp.tool_calls.len(),
                tool_calls_digest,
                usage: resp
                    .usage
                    .as_ref()
                    .map(|u| serde_json::to_value(u).unwrap()),
            });

            match resp.finish_reason {
                FinishReason::Stop => {
                    let text = resp.message.unwrap_or_default();
                    let (red, _) = dep.redactor.redact(&text);
                    let _ = dep
                        .audit
                        .lock()
                        .unwrap()
                        .write(&AuditEvent::AssistantMessage {
                            ts: now_iso(),
                            text_redacted: red.clone(),
                        });
                    if let Err(e) = dep.memory.append_action(&RecentAction {
                        ts: now_iso(),
                        kind: ActionKind::Assistant,
                        summary: red.clone(),
                        detail_json: None,
                    }) {
                        tracing::warn!("memory append_action(assistant) failed: {}", e);
                    }
                    return Ok(LoopResult {
                        assistant_text: Some(text),
                        stopped_reason: "stop".into(),
                    });
                }
                FinishReason::Length => {
                    let _ = dep.audit.lock().unwrap().write(&AuditEvent::Error {
                        ts: now_iso(),
                        code: "llm_response_truncated".into(),
                        message: "model response truncated".into(),
                        context_redacted: None,
                    });
                    return Ok(LoopResult {
                        assistant_text: None,
                        stopped_reason: "length".into(),
                    });
                }
                FinishReason::Refusal => {
                    return Ok(LoopResult {
                        assistant_text: resp.message,
                        stopped_reason: "refusal".into(),
                    });
                }
                FinishReason::Error => {
                    let _ = dep.audit.lock().unwrap().write(&AuditEvent::Error {
                        ts: now_iso(),
                        code: "llm_provider_error".into(),
                        message: "provider error".into(),
                        context_redacted: None,
                    });
                    return Ok(LoopResult {
                        assistant_text: None,
                        stopped_reason: "error".into(),
                    });
                }
                FinishReason::ToolCalls => {
                    if resp.tool_calls.len() > dep.bounds.max_tool_calls_per_iteration as usize {
                        let _ = dep.audit.lock().unwrap().write(&AuditEvent::Error {
                            ts: now_iso(),
                            code: "too_many_tool_calls".into(),
                            message: "model requested too many tool calls".into(),
                            context_redacted: None,
                        });
                        return Ok(LoopResult {
                            assistant_text: None,
                            stopped_reason: "too_many_tool_calls".into(),
                        });
                    }

                    let model_plan = ModelPlan::from(resp);
                    let outcome =
                        dep.pipeline
                            .check(model_plan, &dep.policy_ctx, &dep.sensitive_patterns);

                    if !outcome.schema_errors.is_empty()
                        && schema_attempts < dep.bounds.max_schema_repair_attempts
                    {
                        schema_attempts += 1;
                        for (id, msg) in &outcome.schema_errors {
                            self.builder.append_schema_error(id, msg);
                        }
                        continue;
                    }

                    // Audit ModelPlan + PolicyDecision per step
                    let plan_id = outcome.plan.plan_id.clone();
                    let summary: serde_json::Value = serde_json::Value::Array(
                        outcome
                            .plan
                            .steps
                            .iter()
                            .map(|s| {
                                serde_json::json!({
                                    "step_id": s.call.id,
                                    "tool": s.call.tool_name,
                                })
                            })
                            .collect(),
                    );
                    let _ = dep.audit.lock().unwrap().write(&AuditEvent::ModelPlan {
                        ts: now_iso(),
                        plan_id: plan_id.clone(),
                        steps_digest: canonical_json_digest(&summary),
                        steps_summary: summary,
                    });
                    for s in &outcome.plan.steps {
                        let _ = dep
                            .audit
                            .lock()
                            .unwrap()
                            .write(&AuditEvent::PolicyDecision {
                                ts: now_iso(),
                                plan_id: plan_id.clone(),
                                step_id: s.call.id.clone(),
                                effective_risk: format!("{:?}", s.decision.effective_risk)
                                    .to_lowercase(),
                                action: serde_json::to_value(&s.decision.action)?,
                                flags: s
                                    .decision
                                    .flags
                                    .iter()
                                    .map(|f| format!("{:?}", f))
                                    .collect(),
                                reasons: s.decision.reasons.clone(),
                            });
                    }

                    if outcome.plan.has_deny() {
                        // Surface a synthetic message so the user sees why
                        return Ok(LoopResult {
                            assistant_text: Some(
                                "Action refusée par la politique de sécurité.".into(),
                            ),
                            stopped_reason: "denied".into(),
                        });
                    }

                    if outcome.plan.requires_confirmation() {
                        let granted = dep.gate.ask(&outcome.plan);
                        let _ = dep
                            .audit
                            .lock()
                            .unwrap()
                            .write(&AuditEvent::ConfirmationAsked {
                                ts: now_iso(),
                                plan_id: plan_id.clone(),
                                phrase: outcome.plan.steps.iter().find_map(|s| {
                                    match &s.decision.action {
                                        llmsh_policy::types::PolicyAction::RequireConfirmation {
                                            phrase,
                                            ..
                                        } => phrase.clone(),
                                        _ => None,
                                    }
                                }),
                                granted,
                            });
                        if !granted {
                            self.builder.append_user_cancellation();
                            return Ok(LoopResult {
                                assistant_text: Some("Action annulée.".into()),
                                stopped_reason: "cancelled".into(),
                            });
                        }
                    }

                    // Execute
                    for s in &outcome.plan.steps {
                        let args_digest = canonical_json_digest(&s.call.args);
                        let _ = dep
                            .audit
                            .lock()
                            .unwrap()
                            .write(&AuditEvent::ToolExecutionStart {
                                ts: now_iso(),
                                plan_id: plan_id.clone(),
                                step_id: s.call.id.clone(),
                                tool: s.call.tool_name.clone(),
                                args_digest,
                                args_preview_redacted: Some(redact_args_preview(
                                    &s.call.args,
                                    &dep.redactor,
                                )),
                            });
                    }
                    let results = dep
                        .executor
                        .run_sequential(&outcome.plan, &dep.policy_ctx.cwd)
                        .await;
                    for r in &results {
                        let stdout_red = r
                            .output
                            .as_ref()
                            .map(|o| dep.redactor.redact(&o.stdout).0)
                            .unwrap_or_default();
                        let stderr_red = r
                            .output
                            .as_ref()
                            .and_then(|o| o.stderr.clone())
                            .map(|s| dep.redactor.redact(&s).0);
                        let truncated = r.output.as_ref().map(|o| o.truncated).unwrap_or(false);
                        let _ = dep
                            .audit
                            .lock()
                            .unwrap()
                            .write(&AuditEvent::ToolExecutionEnd {
                                ts: now_iso(),
                                plan_id: plan_id.clone(),
                                step_id: r.step_id.clone(),
                                status: format!("{:?}", r.status).to_lowercase(),
                                exit_code: r.output.as_ref().and_then(|o| o.exit_code),
                                stdout_redacted: stdout_red,
                                stderr_redacted: stderr_red,
                                truncated,
                                duration_ms: r.duration.as_millis() as u64,
                            });
                    }
                    for r in &results {
                        let status = format!("{:?}", r.status).to_lowercase();
                        let summary = format!("{}: {}", r.tool_name, status);
                        let detail = serde_json::json!({
                            "tool": r.tool_name,
                            "status": status,
                            "error": r.error,
                        });
                        if let Err(e) = dep.memory.append_action(&RecentAction {
                            ts: now_iso(),
                            kind: ActionKind::Tool,
                            summary,
                            detail_json: Some(detail.to_string()),
                        }) {
                            tracing::warn!("memory append_action(tool) failed: {}", e);
                        }
                    }
                    self.builder.append_tool_results(&results);
                }
            }
        }
    }
}

fn redact_args_preview(args: &serde_json::Value, r: &Redactor) -> serde_json::Value {
    let s = args.to_string();
    let (red, _) = r.redact(&s);
    serde_json::from_str(&red).unwrap_or(serde_json::Value::String(red))
}
