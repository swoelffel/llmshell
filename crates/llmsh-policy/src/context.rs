use crate::types::{PolicyFlag, RiskLevel};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

/// Shared, mutable working directory across the REPL/agent/executor.
pub type SharedCwd = Arc<RwLock<PathBuf>>;

#[derive(Debug, Clone)]
pub struct PolicyContext {
    pub cwd: SharedCwd,
    pub workspace_root: PathBuf,
    pub allowed_roots: Vec<PathBuf>,
    pub sensitive_path_patterns: Vec<String>,
}

impl PolicyContext {
    /// Read-only snapshot of the current cwd.
    pub fn cwd_snapshot(&self) -> PathBuf {
        self.cwd.read().unwrap().clone()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedPath {
    pub original: String,
    pub canonical: PathBuf,
    pub matches_sensitive: bool,
}

#[derive(Debug, Clone)]
pub struct CheckedToolCall {
    pub id: String,
    pub tool_name: String,
    pub args: Value,
    pub declared_risk: RiskLevel,
    pub resolved_paths: Vec<ResolvedPath>,
    pub flags: Vec<PolicyFlag>,
}
