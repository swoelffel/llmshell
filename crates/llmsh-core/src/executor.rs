use crate::plan::CheckedPlan;
use llmsh_tools::registry::ToolRegistry;
use llmsh_tools::tool::{ToolContext, ToolOutput};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub enum ExecutionStatus {
    Success,
    Failed,
    Cancelled,
    TimedOut,
}

pub struct StepResult {
    pub step_id: String,
    pub tool_name: String,
    pub status: ExecutionStatus,
    pub output: Option<ToolOutput>,
    pub error: Option<String>,
    pub duration: Duration,
}

pub struct ToolExecutor {
    pub registry: Arc<ToolRegistry>,
    pub timeout: Duration,
    pub max_output_bytes: usize,
    pub env: std::collections::HashMap<String, String>,
    pub cancel: CancellationToken,
}

impl ToolExecutor {
    pub async fn run_sequential(
        &self,
        plan: &CheckedPlan,
        cwd: &std::path::Path,
    ) -> Vec<StepResult> {
        let mut out = Vec::new();
        for step in &plan.steps {
            if matches!(
                step.decision.action,
                llmsh_policy::types::PolicyAction::Deny
            ) {
                out.push(StepResult {
                    step_id: step.call.id.clone(),
                    tool_name: step.call.tool_name.clone(),
                    status: ExecutionStatus::Failed,
                    output: None,
                    error: Some(step.decision.reasons.join("; ")),
                    duration: Duration::ZERO,
                });
                continue;
            }
            let tool = match self.registry.get(&step.call.tool_name) {
                Some(t) => t,
                None => {
                    out.push(StepResult {
                        step_id: step.call.id.clone(),
                        tool_name: step.call.tool_name.clone(),
                        status: ExecutionStatus::Failed,
                        output: None,
                        error: Some("tool removed from registry".into()),
                        duration: Duration::ZERO,
                    });
                    continue;
                }
            };
            let ctx = ToolContext {
                cwd: cwd.to_path_buf(),
                timeout: self.timeout,
                env: self.env.clone(),
                max_output_bytes: self.max_output_bytes,
                cancel: self.cancel.clone(),
            };
            let start = Instant::now();
            let res = tool.execute(step.call.args.clone(), &ctx).await;
            let duration = start.elapsed();
            let result = match res {
                Ok(o) => StepResult {
                    step_id: step.call.id.clone(),
                    tool_name: step.call.tool_name.clone(),
                    status: ExecutionStatus::Success,
                    output: Some(o),
                    error: None,
                    duration,
                },
                Err(e) => {
                    let msg = e.to_string();
                    let status = if msg.contains("cancelled") {
                        ExecutionStatus::Cancelled
                    } else if msg.contains("timeout") {
                        ExecutionStatus::TimedOut
                    } else {
                        ExecutionStatus::Failed
                    };
                    StepResult {
                        step_id: step.call.id.clone(),
                        tool_name: step.call.tool_name.clone(),
                        status,
                        output: None,
                        error: Some(msg),
                        duration,
                    }
                }
            };
            out.push(result);
            if self.cancel.is_cancelled() {
                break;
            }
        }
        out
    }
}
