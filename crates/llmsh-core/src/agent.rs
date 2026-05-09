use crate::compactor;
use crate::config::{CompactConfig, MemoryConfig};
use crate::context::{ContextBuilder, SystemPromptSource};
use crate::executor::ToolExecutor;
use crate::memory::{ConversationMessage, Memory};
use crate::pipeline::Pipeline;
use crate::plan::ModelPlan;
use crate::session_stats::SessionStats;
use llmsh_audit::digest::canonical_json_digest;
use llmsh_audit::event::{now_iso, AuditEvent};
use llmsh_audit::redact::Redactor;
use llmsh_audit::writer::AuditWriter;
use llmsh_llm::provider::LlmProvider;
use llmsh_llm::types::{FinishReason, LlmRequest, MessageRole, ToolPolicyHint};
use llmsh_policy::context::PolicyContext;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
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
    pub compact_config: CompactConfig,
    pub memory_cfg: MemoryConfig,
    pub policy_ctx: PolicyContext,
    pub sensitive_patterns: Vec<String>,
    pub model_label: std::sync::Arc<std::sync::RwLock<String>>,
    pub system_prompt: Arc<dyn SystemPromptSource>,
    pub memory: Arc<Memory>,
    /// 0 = silent, 1 = tier-1, 2 = tier-1 + tier-2.
    pub verbose: u8,
    /// Live session stats; shared with the status-line prompt.
    pub stats: Arc<std::sync::RwLock<SessionStats>>,
    /// Last directory before the current `cd` (shared with the REPL).
    pub oldpwd: Arc<Mutex<Option<PathBuf>>>,
    /// User home, used to resolve `cd` / `cd ~` / `cd ~/foo`.
    pub home: Option<PathBuf>,
}

fn persist_from_msg(memory: &Memory, m: &llmsh_llm::types::Message, insert_source: &str) {
    let tcs_json = m
        .tool_calls
        .as_ref()
        .and_then(|tcs| serde_json::to_string(tcs).ok());
    let role = match m.role {
        MessageRole::User => "user",
        MessageRole::Assistant => "assistant",
        MessageRole::Tool => "tool",
        MessageRole::System => "system",
    };
    if let Err(e) = memory.append_message(&ConversationMessage {
        id: 0,
        ts: now_iso(),
        role: role.into(),
        content: m.content.clone(),
        tool_call_id: m.tool_call_id.clone(),
        name: m.name.clone(),
        tool_calls_json: tcs_json,
        insert_source: insert_source.into(),
    }) {
        tracing::warn!("memory.append_message failed: {}", e);
    }
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

        if let Ok(mut s) = dep.stats.write() {
            s.begin_user_turn();
        }

        self.builder.append_user(user_input);
        if let Some(last) = self.builder.messages.last() {
            persist_from_msg(&dep.memory, last, "turn");
        }

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

            // Auto-compaction: triggered by the prior turn's input_tokens
            // crossing the configured threshold for the active model.
            {
                let last_input = dep
                    .stats
                    .read()
                    .ok()
                    .and_then(|s| s.last_turn.as_ref().map(|t| t.input_tokens))
                    .unwrap_or(0);
                let model_now = dep
                    .model_label
                    .read()
                    .map(|g| g.clone())
                    .unwrap_or_else(|_| "unknown".into());
                let window = llmsh_llm::context_window::context_window_for(&model_now);
                let threshold =
                    (window as u64 * dep.compact_config.auto_threshold_pct as u64 / 100) as u32;
                if dep.compact_config.auto_threshold_pct > 0
                    && last_input > 0
                    && last_input >= threshold
                {
                    let report = compactor::compact(
                        &mut self.builder.messages,
                        &dep.compact_config,
                        &dep.memory_cfg,
                        compactor::CompactionReason::Auto,
                        &model_now,
                        last_input,
                        dep.provider.clone(),
                        dep.memory.clone(),
                    )
                    .await;
                    let _ = dep
                        .audit
                        .lock()
                        .unwrap()
                        .write(&AuditEvent::ContextCompacted {
                            ts: now_iso(),
                            reason: report.reason.as_str().into(),
                            strategy: report.strategy.as_str().into(),
                            messages_before: report.messages_before,
                            messages_after: report.messages_after,
                            bytes_before: report.bytes_before,
                            bytes_after: report.bytes_after,
                            summary_digest: report.summary_digest,
                        });
                }
            }

            let req = LlmRequest {
                system: Some(dep.system_prompt.current()),
                messages: self.builder.messages.clone(),
                tools: dep.pipeline.registry.specs(),
                tool_policy: ToolPolicyHint::PreferTools,
                response_format: None,
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
                    // Append the final assistant reply so the persisted builder
                    // retains the full conversation history for subsequent turns.
                    self.builder.append_assistant(&text);
                    if let Some(last) = self.builder.messages.last() {
                        persist_from_msg(&dep.memory, last, "turn");
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

                    // Snapshot the assistant turn so the wire payload includes
                    // the assistant message with tool_calls — required by
                    // OpenAI: every `tool` role message must follow such an
                    // assistant message.
                    let assistant_text = resp.message.clone();
                    let assistant_tool_calls = resp.tool_calls.clone();

                    let model_plan = ModelPlan::from(resp);
                    let outcome =
                        dep.pipeline
                            .check(model_plan, &dep.policy_ctx, &dep.sensitive_patterns);

                    // Append the assistant turn that triggered the tool calls,
                    // before any `tool` messages get appended (either via
                    // schema-repair or via execution). OpenAI requires this
                    // ordering on the wire.
                    self.builder.append_assistant_with_tool_calls(
                        assistant_text.as_deref(),
                        assistant_tool_calls.clone(),
                    );
                    if let Some(last) = self.builder.messages.last() {
                        persist_from_msg(&dep.memory, last, "turn");
                    }

                    if !outcome.schema_errors.is_empty()
                        && schema_attempts < dep.bounds.max_schema_repair_attempts
                    {
                        schema_attempts += 1;
                        for (id, msg) in &outcome.schema_errors {
                            self.builder.append_schema_error(id, msg);
                        }
                        if let Ok(mut s) = dep.stats.write() {
                            if let Some(t) = s.last_turn.as_mut() {
                                t.schema_repair_attempts = schema_attempts;
                            }
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
                        // Synthesize tool-result messages for every step so
                        // the conversation contains a `tool` reply for each
                        // tool_call_id that was emitted by the assistant.
                        // Without this, OpenAI returns 400 the next turn.
                        for s in &outcome.plan.steps {
                            self.builder.append_tool_denied(
                                &s.call.id,
                                &s.call.tool_name,
                                "policy denied",
                            );
                            if let Some(last) = self.builder.messages.last() {
                                persist_from_msg(&dep.memory, last, "turn");
                            }
                        }
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
                            for s in &outcome.plan.steps {
                                self.builder.append_tool_denied(
                                    &s.call.id,
                                    &s.call.tool_name,
                                    "user cancelled",
                                );
                                if let Some(last) = self.builder.messages.last() {
                                    persist_from_msg(&dep.memory, last, "turn");
                                }
                            }
                            return Ok(LoopResult {
                                assistant_text: Some("Action annulée.".into()),
                                stopped_reason: "cancelled".into(),
                            });
                        }
                    }

                    // Pre-pass: identify run_process(cd, ...) steps so we can
                    // handle them in-process (subprocess `cd` would die with
                    // the child, leaving the parent PWD unchanged).
                    let mut cd_indices: Vec<usize> = Vec::new();
                    for (i, s) in outcome.plan.steps.iter().enumerate() {
                        if s.call.tool_name == "run_process" {
                            let prog = s.call.args.get("program").and_then(|v| v.as_str());
                            if prog == Some("cd") {
                                cd_indices.push(i);
                            }
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
                    // Build a sub-plan for the executor that excludes cd steps.
                    let mut other_plan = outcome.plan.clone();
                    other_plan.steps = outcome
                        .plan
                        .steps
                        .iter()
                        .enumerate()
                        .filter(|(i, _)| !cd_indices.contains(i))
                        .map(|(_, s)| s.clone())
                        .collect();
                    let exec_cwd = dep.policy_ctx.cwd_snapshot();
                    let mut exec_results = if other_plan.steps.is_empty() {
                        Vec::new()
                    } else {
                        dep.executor.run_sequential(&other_plan, &exec_cwd).await
                    };
                    // Now build cd synthetic results and merge in plan order.
                    let mut results: Vec<crate::executor::StepResult> =
                        Vec::with_capacity(outcome.plan.steps.len());
                    let mut exec_iter = exec_results.drain(..);
                    for (i, step) in outcome.plan.steps.iter().enumerate() {
                        if cd_indices.contains(&i) {
                            let arg = step
                                .call
                                .args
                                .get("args")
                                .and_then(|v| v.as_array())
                                .and_then(|a| a.first())
                                .and_then(|v| v.as_str());
                            let from = dep.policy_ctx.cwd_snapshot();
                            let oldpwd = dep.oldpwd.lock().unwrap().clone();
                            let target = crate::cwd::resolve_cd_target(
                                arg,
                                &from,
                                dep.home.as_deref(),
                                oldpwd.as_deref(),
                            );
                            let started = Instant::now();
                            let res = match target {
                                Ok(t) => crate::cwd::try_chdir(&dep.policy_ctx.cwd, &t),
                                Err(e) => Err(e),
                            };
                            let duration = started.elapsed();
                            match res {
                                Ok(new) => {
                                    *dep.oldpwd.lock().unwrap() = Some(from.clone());
                                    let _ =
                                        dep.audit.lock().unwrap().write(&AuditEvent::CwdChanged {
                                            ts: now_iso(),
                                            from: from.display().to_string(),
                                            to: new.display().to_string(),
                                            source: "tool".into(),
                                        });
                                    results.push(crate::executor::StepResult {
                                        step_id: step.call.id.clone(),
                                        tool_name: step.call.tool_name.clone(),
                                        status: crate::executor::ExecutionStatus::Success,
                                        output: Some(llmsh_tools::tool::ToolOutput {
                                            stdout: format!("cwd: {}", new.display()),
                                            stderr: None,
                                            exit_code: Some(0),
                                            truncated: false,
                                            structured: None,
                                        }),
                                        error: None,
                                        duration,
                                    });
                                }
                                Err(e) => {
                                    results.push(crate::executor::StepResult {
                                        step_id: step.call.id.clone(),
                                        tool_name: step.call.tool_name.clone(),
                                        status: crate::executor::ExecutionStatus::Failed,
                                        output: None,
                                        error: Some(format!("cd: {}", e)),
                                        duration,
                                    });
                                }
                            }
                        } else if let Some(r) = exec_iter.next() {
                            results.push(r);
                        }
                    }
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
                    {
                        let plan_steps = &outcome.plan.steps;
                        if let Ok(mut s) = dep.stats.write() {
                            if let Some(t) = s.last_turn.as_mut() {
                                for r in &results {
                                    let step =
                                        plan_steps.iter().find(|step| step.call.id == r.step_id);
                                    let (risk, flags) = match step {
                                        Some(p) => (
                                            format!("{:?}", p.decision.effective_risk),
                                            p.decision
                                                .flags
                                                .iter()
                                                .map(|f| format!("{:?}", f))
                                                .collect(),
                                        ),
                                        None => ("Unknown".to_string(), vec![]),
                                    };
                                    let bytes =
                                        r.output.as_ref().map(|o| o.stdout.len()).unwrap_or(0);
                                    t.tool_steps.push(crate::session_stats::ToolStepStats {
                                        tool: r.tool_name.clone(),
                                        status: format!("{:?}", r.status).to_lowercase(),
                                        duration: r.duration,
                                        output_bytes: bytes,
                                        risk,
                                        flags,
                                    });
                                }
                            }
                        }
                    }
                    let n_results = results.len();
                    self.builder.append_tool_results(&results);
                    let total = self.builder.messages.len();
                    for m in self.builder.messages[total.saturating_sub(n_results)..].iter() {
                        persist_from_msg(&dep.memory, m, "turn");
                    }
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
