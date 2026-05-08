use crate::agent::{AgentDeps, AgentLoop};
use crate::context::ContextBuilder;
use crate::init::MachineAudit;
use crate::input::{classify, InputKind};
use crate::model_cmd::{handle_model_command, ModelCommandContext, ModelListCache};
use crate::raw_shell::{resolve_shell, RiskScan};
use llmsh_audit::event::{now_iso, AuditEvent};
use reedline::{DefaultPrompt, Reedline, Signal};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

pub struct ReplState {
    pub cwd: std::path::PathBuf,
    pub workspace_root: std::path::PathBuf,
    pub allowed_roots: Vec<std::path::PathBuf>,
    pub history_recent: Vec<String>,
}

pub struct Repl {
    pub deps: Arc<AgentDeps>,
    pub state: ReplState,
    pub max_llm_output_bytes: usize,
    pub raw_shell: Option<String>,
    pub risk_scan: RiskScan,
    pub root_cancel: CancellationToken,
    pub config_path: Option<PathBuf>,
    pub model_cache: ModelListCache,
}

impl Repl {
    pub async fn run(mut self) -> anyhow::Result<()> {
        let mut line_editor = Reedline::create();
        let prompt = DefaultPrompt::default();
        loop {
            match line_editor.read_line(&prompt)? {
                Signal::Success(line) => {
                    let _ = self
                        .deps
                        .audit
                        .lock()
                        .unwrap()
                        .write(&AuditEvent::UserInput {
                            ts: now_iso(),
                            kind: kind_label(&line),
                            text_redacted: self.deps.redactor.redact(&line).0,
                        });
                    self.state.history_recent.push(line.clone());
                    match classify(&line) {
                        InputKind::Empty => continue,
                        InputKind::Meta(cmd, args) => {
                            if cmd == "exit" {
                                break;
                            }
                            self.handle_meta(&cmd, &args).await?;
                        }
                        InputKind::RawShell(c) => self.handle_raw_shell(&c).await?,
                        InputKind::Natural(t) => {
                            let mut loop_state = AgentLoop {
                                deps: self.deps.clone(),
                                builder: ContextBuilder::new(self.max_llm_output_bytes),
                            };
                            match loop_state.run(&t).await {
                                Ok(r) => {
                                    if let Some(text) = r.assistant_text {
                                        println!("{}", text);
                                    }
                                }
                                Err(e) => eprintln!("error: {}", e),
                            }
                        }
                    }
                    let _ = self.deps.audit.lock().unwrap().flush();
                }
                Signal::CtrlC => {
                    self.root_cancel.cancel();
                    println!("(cancelled)");
                }
                Signal::CtrlD => {
                    break;
                }
            }
        }
        let _ = self
            .deps
            .audit
            .lock()
            .unwrap()
            .write(&AuditEvent::SessionEnded {
                ts: now_iso(),
                reason: "user_exit".into(),
            });
        let _ = self.deps.audit.lock().unwrap().flush();
        Ok(())
    }

    async fn handle_meta(&mut self, cmd: &str, args: &[String]) -> anyhow::Result<()> {
        match cmd {
            "help" => {
                println!("/help /exit /pwd /cd <path> /history /model [list|set <id>] /init");
            }
            "init" => {
                let audit = MachineAudit::capture_with_tooling().await;
                let host = audit.identity_host().to_string();
                let os = audit.identity_os();
                let user = audit.identity_user().to_string();
                let tooling_count = audit.tooling_count();
                let short = audit.render_short_summary();
                let init_audit = audit.into_init_audit();
                if let Err(e) = self.deps.memory.write_init_audit(&init_audit) {
                    eprintln!("init: failed to write audit: {}", e);
                } else {
                    println!("{}", short);
                    let _ =
                        self.deps
                            .audit
                            .lock()
                            .unwrap()
                            .write(&AuditEvent::MachineAuditPerformed {
                                ts: now_iso(),
                                host,
                                os,
                                user,
                                tooling_count,
                            });
                }
            }
            "pwd" => println!("{}", self.state.cwd.display()),
            "cd" => {
                if let Some(p) = args.first() {
                    let target = if std::path::Path::new(p).is_absolute() {
                        std::path::PathBuf::from(p)
                    } else {
                        self.state.cwd.join(p)
                    };
                    let canonical = std::fs::canonicalize(&target).unwrap_or(target);
                    let inside = self.state.allowed_roots.iter().any(|r| {
                        let cr = std::fs::canonicalize(r).unwrap_or(r.clone());
                        canonical.starts_with(cr)
                    });
                    if inside {
                        self.state.cwd = canonical;
                    } else {
                        eprintln!(
                            "/cd refused: outside allowed_roots (use /allow-root in a future version)"
                        );
                    }
                }
            }
            "history" => {
                for h in self.state.history_recent.iter().rev().take(20).rev() {
                    println!("{}", h);
                }
            }
            "model" => {
                let ctx = ModelCommandContext {
                    provider: self.deps.provider.as_ref(),
                    model_label: &self.deps.model_label,
                    cache: &self.model_cache,
                    config_path: self.config_path.as_deref(),
                    audit: &self.deps.audit,
                };
                if let Err(e) = handle_model_command(&ctx, args).await {
                    eprintln!("model command error: {}", e);
                }
            }
            other => eprintln!("unknown meta command: /{}", other),
        }
        Ok(())
    }

    async fn handle_raw_shell(&mut self, command: &str) -> anyhow::Result<()> {
        let hits = self.risk_scan.scan(command);
        if !hits.is_empty() {
            println!("⚠ critical patterns detected: {}", hits.join(", "));
            print!("Type 'yes' to proceed: ");
            let _ = std::io::stdout().flush();
            let mut line = String::new();
            std::io::stdin().read_line(&mut line)?;
            if line.trim() != "yes" {
                println!("aborted");
                return Ok(());
            }
        }
        let (shell, sargs) = resolve_shell(&self.raw_shell);
        let start = Instant::now();
        let mut cmd = Command::new(&shell);
        for a in &sargs {
            cmd.arg(a);
        }
        cmd.arg(command)
            .current_dir(&self.state.cwd)
            .kill_on_drop(true);
        let cancel = self.root_cancel.clone();
        let child = cmd.spawn()?;
        let mut output_task = tokio::spawn(child.wait_with_output());
        let output = tokio::select! {
            _ = cancel.cancelled() => {
                output_task.abort();
                None
            }
            res = &mut output_task => {
                match res {
                    Ok(Ok(o)) => Some(o),
                    _ => None,
                }
            }
        };
        let duration = start.elapsed();
        let (status, exit, stdout, stderr) = match output {
            Some(o) => (
                if o.status.success() {
                    "success"
                } else {
                    "failed"
                },
                o.status.code(),
                String::from_utf8_lossy(&o.stdout).to_string(),
                String::from_utf8_lossy(&o.stderr).to_string(),
            ),
            None => ("cancelled", None, String::new(), String::new()),
        };
        if !stdout.is_empty() {
            print!("{}", stdout);
        }
        if !stderr.is_empty() {
            eprint!("{}", stderr);
        }
        let cmd_red = self.deps.redactor.redact(command).0;
        let stdout_red = self.deps.redactor.redact(&stdout).0;
        let stderr_red = self.deps.redactor.redact(&stderr).0;
        let _ = self
            .deps
            .audit
            .lock()
            .unwrap()
            .write(&AuditEvent::RawShellExecution {
                ts: now_iso(),
                command_redacted: cmd_red,
                status: status.into(),
                exit_code: exit,
                stdout_redacted: stdout_red,
                stderr_redacted: Some(stderr_red),
                truncated: false,
                risk_scan_hits: hits,
                duration_ms: duration.as_millis() as u64,
            });
        Ok(())
    }
}

fn kind_label(line: &str) -> String {
    let t = line.trim_start();
    if t.starts_with('!') {
        "raw".into()
    } else if t.starts_with('/') {
        "meta".into()
    } else {
        "natural".into()
    }
}
