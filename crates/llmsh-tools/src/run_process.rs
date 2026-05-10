use crate::path_util::expand_tilde;
use crate::tool::{Tool, ToolCategory, ToolContext, ToolOutput};
use async_trait::async_trait;
use llmsh_policy::types::RiskLevel;
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::process::Command;

#[derive(Deserialize)]
struct Args {
    program: String,
    #[serde(default)]
    args: Vec<String>,
    cwd: Option<String>,
    timeout_ms: Option<u64>,
    /// One-line natural-language description of WHY this command is run.
    /// Audit-only; surfaced in the confirmation prompt.
    #[serde(default)]
    #[allow(dead_code)]
    intent: Option<String>,
    /// LLM's own estimate of the risk level — used as an upgrade-only hint
    /// by the policy engine.
    #[serde(default)]
    #[allow(dead_code)]
    claimed_risk: Option<String>,
}

pub struct RunProcess;

#[async_trait]
impl Tool for RunProcess {
    fn name(&self) -> &str {
        "run_process"
    }
    fn description(&self) -> &str {
        "Run a program with arguments. No shell is invoked: `~` and `~/…` are \
expanded against $HOME, but globs (`*`, `?`, `[]`) and environment variables \
(`$VAR`, `${VAR}`) are NOT — they are passed literally. To use a glob, call \
the `glob` tool first and pass the resolved paths as separate args.\n\
\n\
SHELL USAGE — important.\n\
Use `program` + `args` directly whenever possible. Wrap in `bash -c \"…\"`\n\
ONLY when you genuinely need:\n\
  • a pipe (cmd1 | cmd2)\n\
  • redirection (>, <, <<)\n\
  • variable expansion ($VAR, $(cmd))\n\
  • subshell or compound logic (&&, ||, ;)\n\
\n\
Examples:\n\
  ❌  program=bash, args=[\"-c\", \"ls -la /tmp\"]\n\
  ✅  program=ls,   args=[\"-la\", \"/tmp\"]\n\
\n\
  ❌  program=bash, args=[\"-c\", \"dscl . list /Users\"]\n\
  ✅  program=dscl, args=[\".\", \"list\", \"/Users\"]\n\
\n\
  ❌  program=bash, args=[\"-c\", \"grep TODO src/main.rs\"]\n\
  ✅  program=grep, args=[\"TODO\", \"src/main.rs\"]\n\
\n\
  ✅  program=bash, args=[\"-c\", \"ls /tmp | wc -l\"]   (pipe — legitimate)\n\
  ✅  program=bash, args=[\"-c\", \"echo $HOME\"]        (var — legitimate)\n\
\n\
For `*` / `?` patterns, call the `glob` tool FIRST to expand the pattern,\n\
then pass the resulting paths to `program` directly.\n\
\n\
Why it matters: the policy classifier inspects `program` + `args` to grant\n\
read-only commands `Allow` (no prompt). Wrapping in bash blinds the\n\
classifier — every command then requires user confirmation, which is\n\
friction the user dislikes.\n\
\n\
claimed_risk — your honest estimate (the policy may override):\n\
  • read_only   — no filesystem/network/state mutation (ls, grep, cat…)\n\
  • low_risk    — touches state but reversibly (mkdir, touch, cd…)\n\
  • write       — creates or modifies files (cp, mv, edit…)\n\
  • destructive — irreversible loss possible (rm -rf, dd, format…)\n\
  • network     — outbound network call (curl, wget, ssh…)\n\
  • privileged  — requires root (sudo, doas, anything in /etc, /System…)"
    }
    fn input_schema(&self) -> Value {
        json!({
            "type":"object",
            "properties": {
                "intent": {
                    "type":"string",
                    "description":"One-line description of WHY this command is run. Required. Write this BEFORE program/args so you reread the goal as you build the command."
                },
                "claimed_risk": {
                    "type":"string",
                    "enum": ["read_only","low","write","destructive","network","privileged","unknown"],
                    "description":"Your honest estimate of the risk this command poses. The engine only uses this to RAISE risk, never to lower it — declaring read_only does not skip confirmation."
                },
                "program": {"type":"string"},
                "args": {"type":"array","items":{"type":"string"}},
                "cwd": {"type":"string"},
                "timeout_ms": {"type":"integer","minimum":1}
            },
            "required":["intent","claimed_risk","program"],
            "additionalProperties": false
        })
    }
    fn declared_risk(&self) -> RiskLevel {
        RiskLevel::Unknown
    }
    fn category(&self) -> ToolCategory {
        ToolCategory::Process
    }

    async fn execute(&self, args: Value, ctx: &ToolContext) -> anyhow::Result<ToolOutput> {
        let a: Args = serde_json::from_value(args)?;
        // Tilde-expand the program path (only if it starts with `~`); otherwise
        // hand the original string to `Command::new` so PATH lookup still works.
        let program: std::borrow::Cow<'_, str> = if a.program.starts_with('~') {
            std::borrow::Cow::Owned(
                expand_tilde(&a.program, ctx.home.as_deref())
                    .to_string_lossy()
                    .into_owned(),
            )
        } else {
            std::borrow::Cow::Borrowed(a.program.as_str())
        };
        let mut cmd = Command::new(program.as_ref());
        // Tilde-expand each arg; everything else (globs, $VARs) stays literal.
        let expanded_args: Vec<String> = a
            .args
            .iter()
            .map(|raw| {
                if raw.starts_with('~') {
                    expand_tilde(raw, ctx.home.as_deref())
                        .to_string_lossy()
                        .into_owned()
                } else {
                    raw.clone()
                }
            })
            .collect();
        cmd.args(&expanded_args);
        if let Some(c) = &a.cwd {
            let expanded = expand_tilde(c, ctx.home.as_deref());
            let p = if expanded.is_absolute() {
                expanded
            } else {
                ctx.cwd.join(&expanded)
            };
            cmd.current_dir(p);
        } else {
            cmd.current_dir(&ctx.cwd);
        }
        cmd.env_clear();
        for (k, v) in &ctx.env {
            cmd.env(k, v);
        }
        cmd.stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);

        let to = a
            .timeout_ms
            .map(Duration::from_millis)
            .unwrap_or(ctx.timeout);
        let child = cmd.spawn()?;
        let cancel = ctx.cancel.clone();
        let output = tokio::select! {
            _ = cancel.cancelled() => {
                anyhow::bail!("cancelled");
            }
            _ = tokio::time::sleep(to) => {
                anyhow::bail!("timeout after {}ms", to.as_millis());
            }
            o = async { child.wait_with_output().await } => { o? }
        };
        let mut stdout = String::from_utf8_lossy(&output.stdout).to_string();
        let mut stderr = String::from_utf8_lossy(&output.stderr).to_string();
        let mut truncated = false;
        if stdout.len() > ctx.max_output_bytes {
            stdout.truncate(ctx.max_output_bytes);
            truncated = true;
        }
        if stderr.len() > ctx.max_output_bytes {
            stderr.truncate(ctx.max_output_bytes);
            truncated = true;
        }
        Ok(ToolOutput {
            stdout,
            structured: None,
            stderr: Some(stderr),
            exit_code: output.status.code(),
            truncated,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tokio_util::sync::CancellationToken;

    fn ctx() -> ToolContext {
        ToolContext {
            cwd: std::env::temp_dir(),
            timeout: Duration::from_secs(5),
            env: HashMap::new(),
            max_output_bytes: 4096,
            cancel: CancellationToken::new(),
            home: None,
        }
    }

    #[tokio::test]
    async fn echoes_no_shell() {
        let t = RunProcess;
        let out = t
            .execute(json!({"program":"echo","args":["hello $HOME"]}), &ctx())
            .await
            .unwrap();
        assert!(out.stdout.contains("hello $HOME")); // not expanded
    }

    #[tokio::test]
    async fn tilde_cwd_is_expanded() {
        let tmp = tempfile::tempdir().unwrap();
        // Pretend the tempdir is $HOME and ask `pwd` to print "~"
        let mut c = ctx();
        c.home = Some(tmp.path().to_path_buf());
        let t = RunProcess;
        let out = t
            .execute(json!({"program":"pwd","cwd":"~"}), &c)
            .await
            .unwrap();
        let printed = out.stdout.trim();
        // Resolve any symlinks (macOS /var → /private/var) on both sides.
        let want = std::fs::canonicalize(tmp.path()).unwrap();
        let got = std::fs::canonicalize(printed).unwrap();
        assert_eq!(got, want);
    }

    #[tokio::test]
    async fn tilde_arg_is_expanded() {
        let tmp = tempfile::tempdir().unwrap();
        let mut c = ctx();
        c.home = Some(tmp.path().to_path_buf());
        let t = RunProcess;
        let out = t
            .execute(json!({"program":"echo","args":["~/foo"]}), &c)
            .await
            .unwrap();
        let want = format!("{}/foo", tmp.path().display());
        assert_eq!(out.stdout.trim(), want);
    }

    #[tokio::test]
    async fn cancellation_fires() {
        let mut c = ctx();
        c.timeout = Duration::from_secs(30);
        c.cancel.cancel();
        let t = RunProcess;
        let r = t
            .execute(json!({"program":"sleep","args":["10"]}), &c)
            .await;
        assert!(r.is_err());
    }
}
