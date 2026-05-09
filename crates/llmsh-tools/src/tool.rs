use async_trait::async_trait;
use llmsh_policy::types::RiskLevel;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolCategory {
    Filesystem,
    Process,
    Git,
    Network,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolOutput {
    pub stdout: String,
    pub structured: Option<Value>,
    pub stderr: Option<String>,
    pub exit_code: Option<i32>,
    pub truncated: bool,
}

#[derive(Debug, Clone)]
pub struct ToolContext {
    pub cwd: PathBuf,
    pub timeout: Duration,
    pub env: HashMap<String, String>,
    pub max_output_bytes: usize,
    pub cancel: tokio_util::sync::CancellationToken,
    /// Resolved `$HOME` for tilde expansion. Tools must not fall back to
    /// `std::env::var("HOME")` — leaving this `None` means tildes stay literal.
    pub home: Option<PathBuf>,
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn input_schema(&self) -> Value;
    fn declared_risk(&self) -> RiskLevel;
    fn category(&self) -> ToolCategory;
    async fn execute(&self, args: Value, ctx: &ToolContext) -> anyhow::Result<ToolOutput>;
}
