use crate::tool::{Tool, ToolCategory, ToolContext, ToolOutput};
use async_trait::async_trait;
use llmsh_policy::types::RiskLevel;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
struct Args {
    path: String,
    max_depth: Option<usize>,
}

pub struct ListDirectory;

#[async_trait]
impl Tool for ListDirectory {
    fn name(&self) -> &str {
        "list_directory"
    }
    fn description(&self) -> &str {
        "List files and directories at the given path."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type":"object",
            "properties": {
                "path": {"type":"string"},
                "max_depth": {"type":"integer","minimum":1}
            },
            "required":["path"],
            "additionalProperties": false
        })
    }
    fn declared_risk(&self) -> RiskLevel {
        RiskLevel::ReadOnly
    }
    fn category(&self) -> ToolCategory {
        ToolCategory::Filesystem
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> anyhow::Result<ToolOutput> {
        let a: Args = serde_json::from_value(args)?;
        let base = if std::path::Path::new(&a.path).is_absolute() {
            std::path::PathBuf::from(&a.path)
        } else {
            ctx.cwd.join(&a.path)
        };
        let depth = a.max_depth.unwrap_or(1).clamp(1, 8);
        let mut out = String::new();
        let mut entries: Vec<Value> = Vec::new();
        let mut total = 0usize;
        let mut truncated = false;
        for e in walkdir::WalkDir::new(&base)
            .max_depth(depth)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let rel = e
                .path()
                .strip_prefix(&base)
                .unwrap_or(e.path())
                .display()
                .to_string();
            let kind = if e.file_type().is_dir() {
                "dir"
            } else {
                "file"
            };
            let line = format!("{} {}\n", kind, rel);
            total += line.len();
            if total > ctx.max_output_bytes {
                truncated = true;
                break;
            }
            out.push_str(&line);
            entries.push(json!({"path": rel, "kind": kind}));
        }
        Ok(ToolOutput {
            stdout: out,
            structured: Some(json!({"entries": entries})),
            stderr: None,
            exit_code: Some(0),
            truncated,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;

    fn ctx(cwd: std::path::PathBuf) -> ToolContext {
        ToolContext {
            cwd,
            timeout: Duration::from_secs(5),
            env: HashMap::new(),
            max_output_bytes: 4096,
            cancel: CancellationToken::new(),
        }
    }

    #[tokio::test]
    async fn lists_temp_dir() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("a.txt"), "x").unwrap();
        let t = ListDirectory;
        let out = t
            .execute(json!({"path":"."}), &ctx(tmp.path().to_path_buf()))
            .await
            .unwrap();
        assert!(out.stdout.contains("a.txt"));
    }
}
