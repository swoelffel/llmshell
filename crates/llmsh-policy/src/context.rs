use crate::types::{PolicyFlag, RiskLevel};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct PolicyContext {
    pub cwd: PathBuf,
    pub workspace_root: PathBuf,
    pub allowed_roots: Vec<PathBuf>,
    pub sensitive_path_patterns: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedPath {
    pub original: String,
    pub canonical: PathBuf,
    pub inside_workspace: bool,
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
