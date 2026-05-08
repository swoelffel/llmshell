use crate::tool::{Tool, ToolCategory, ToolContext, ToolOutput};
use async_trait::async_trait;
use llmsh_policy::types::RiskLevel;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::AsyncReadExt;

#[derive(Deserialize)]
struct Range { start: usize, end: usize }
#[derive(Deserialize)]
struct Args { path: String, range: Option<Range> }

pub struct ReadFile;

#[async_trait]
impl Tool for ReadFile {
    fn name(&self) -> &str { "read_file" }
    fn description(&self) -> &str { "Read the contents of a file. Optional byte range." }
    fn input_schema(&self) -> Value {
        json!({
            "type":"object",
            "properties": {
                "path": {"type":"string"},
                "range": {"type":"object","properties": {"start":{"type":"integer"},"end":{"type":"integer"}},"required":["start","end"]}
            },
            "required":["path"],
            "additionalProperties": false
        })
    }
    fn declared_risk(&self) -> RiskLevel { RiskLevel::ReadOnly }
    fn category(&self) -> ToolCategory { ToolCategory::Filesystem }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> anyhow::Result<ToolOutput> {
        let a: Args = serde_json::from_value(args)?;
        let p = if std::path::Path::new(&a.path).is_absolute() {
            std::path::PathBuf::from(&a.path)
        } else { ctx.cwd.join(&a.path) };
        let mut f = tokio::fs::File::open(&p).await?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).await?;
        let slice: &[u8] = match a.range {
            Some(r) => {
                let end = r.end.min(buf.len());
                let start = r.start.min(end);
                &buf[start..end]
            }
            None => &buf,
        };
        let mut truncated = false;
        let bytes = if slice.len() > ctx.max_output_bytes {
            truncated = true;
            &slice[..ctx.max_output_bytes]
        } else { slice };
        let s = String::from_utf8_lossy(bytes).to_string();
        Ok(ToolOutput { stdout: s, structured: None, stderr: None, exit_code: Some(0), truncated })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::time::Duration;
    use tokio_util::sync::CancellationToken;

    fn ctx(cwd: std::path::PathBuf) -> ToolContext {
        ToolContext { cwd, timeout: Duration::from_secs(5), env: HashMap::new(),
            max_output_bytes: 4096, cancel: CancellationToken::new() }
    }

    #[tokio::test]
    async fn reads_file() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("f.txt");
        std::fs::write(&p, "hello").unwrap();
        let t = ReadFile;
        let out = t.execute(json!({"path":"f.txt"}), &ctx(tmp.path().to_path_buf())).await.unwrap();
        assert_eq!(out.stdout, "hello");
    }

    #[tokio::test]
    async fn truncates() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("big.txt");
        std::fs::write(&p, "x".repeat(10_000)).unwrap();
        let t = ReadFile;
        let mut c = ctx(tmp.path().to_path_buf()); c.max_output_bytes = 100;
        let out = t.execute(json!({"path":"big.txt"}), &c).await.unwrap();
        assert!(out.truncated); assert_eq!(out.stdout.len(), 100);
    }
}
