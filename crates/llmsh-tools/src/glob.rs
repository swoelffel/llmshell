use crate::path_util::expand_tilde;
use crate::tool::{Tool, ToolCategory, ToolContext, ToolOutput};
use async_trait::async_trait;
use llmsh_policy::types::RiskLevel;
use serde::Deserialize;
use serde_json::{json, Value};

/// Hard cap on returned matches. Above this we set `truncated=true` and stop.
const MAX_MATCHES: usize = 1000;

#[derive(Deserialize)]
struct Args {
    pattern: String,
    cwd: Option<String>,
}

pub struct Glob;

#[async_trait]
impl Tool for Glob {
    fn name(&self) -> &str {
        "glob"
    }
    fn description(&self) -> &str {
        "Resolve a shell-style glob pattern (e.g. `*.rs`, `~/Library/Caches/*`, \
`src/**/*.toml`) to absolute file paths. Read-only. `~` and `~/…` are expanded; \
relative patterns are joined with `cwd` (defaulting to the current working \
directory). Returns up to 1000 paths and sets `truncated=true` if more exist. \
Use this before `run_process` whenever you would otherwise rely on shell glob \
expansion."
    }
    fn input_schema(&self) -> Value {
        json!({
            "type":"object",
            "properties": {
                "pattern": {"type":"string"},
                "cwd": {"type":"string"}
            },
            "required":["pattern"],
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
        let base_cwd = match &a.cwd {
            Some(c) => {
                let expanded = expand_tilde(c, ctx.home.as_deref());
                if expanded.is_absolute() {
                    expanded
                } else {
                    ctx.cwd.join(&expanded)
                }
            }
            None => ctx.cwd.clone(),
        };

        let pat_path = expand_tilde(&a.pattern, ctx.home.as_deref());
        let resolved_pattern = if pat_path.is_absolute() {
            pat_path
        } else {
            base_cwd.join(&pat_path)
        };
        let pattern_str = resolved_pattern.to_string_lossy();

        let entries =
            ::glob::glob(&pattern_str).map_err(|e| anyhow::anyhow!("invalid glob pattern: {e}"))?;

        let mut matches: Vec<String> = Vec::new();
        let mut truncated = false;
        for entry in entries {
            match entry {
                Ok(path) => {
                    if matches.len() >= MAX_MATCHES {
                        truncated = true;
                        break;
                    }
                    let abs = if path.is_absolute() {
                        path
                    } else {
                        base_cwd.join(path)
                    };
                    matches.push(abs.to_string_lossy().into_owned());
                }
                Err(_) => continue,
            }
        }

        let count = matches.len();
        let mut stdout = String::new();
        for m in &matches {
            stdout.push_str(m);
            stdout.push('\n');
        }
        if stdout.len() > ctx.max_output_bytes {
            stdout.truncate(ctx.max_output_bytes);
            truncated = true;
        }

        let structured = json!({
            "matches": matches,
            "count": count,
            "truncated": truncated,
        });

        Ok(ToolOutput {
            stdout,
            structured: Some(structured),
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
            max_output_bytes: 65_536,
            cancel: CancellationToken::new(),
            home: None,
        }
    }

    #[tokio::test]
    async fn matches_existing_files() {
        let tmp = tempfile::tempdir().unwrap();
        for name in ["a.txt", "b.txt", "c.md"] {
            std::fs::write(tmp.path().join(name), "x").unwrap();
        }
        let t = Glob;
        let out = t
            .execute(json!({"pattern":"*.txt"}), &ctx(tmp.path().to_path_buf()))
            .await
            .unwrap();
        let s = out.structured.unwrap();
        assert_eq!(s["count"].as_u64().unwrap(), 2);
        assert!(!s["truncated"].as_bool().unwrap());
        let matches = s["matches"].as_array().unwrap();
        let names: Vec<String> = matches
            .iter()
            .map(|v| {
                std::path::Path::new(v.as_str().unwrap())
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        assert!(names.contains(&"a.txt".to_string()));
        assert!(names.contains(&"b.txt".to_string()));
        assert!(!names.contains(&"c.md".to_string()));
    }

    #[tokio::test]
    async fn returns_absolute_paths() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("f.rs"), "x").unwrap();
        let t = Glob;
        let out = t
            .execute(json!({"pattern":"*.rs"}), &ctx(tmp.path().to_path_buf()))
            .await
            .unwrap();
        let matches = out.structured.unwrap()["matches"].clone();
        let first = matches[0].as_str().unwrap();
        assert!(std::path::Path::new(first).is_absolute());
    }

    #[tokio::test]
    async fn tilde_pattern_expands() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("hit.log"), "x").unwrap();
        let mut c = ctx(std::env::temp_dir());
        c.home = Some(tmp.path().to_path_buf());
        let t = Glob;
        let out = t.execute(json!({"pattern":"~/*.log"}), &c).await.unwrap();
        assert_eq!(out.structured.unwrap()["count"].as_u64().unwrap(), 1);
    }

    #[tokio::test]
    async fn no_matches_is_empty_not_error() {
        let tmp = tempfile::tempdir().unwrap();
        let t = Glob;
        let out = t
            .execute(
                json!({"pattern":"*.nothing"}),
                &ctx(tmp.path().to_path_buf()),
            )
            .await
            .unwrap();
        let s = out.structured.unwrap();
        assert_eq!(s["count"].as_u64().unwrap(), 0);
        assert!(!s["truncated"].as_bool().unwrap());
    }
}
