use crate::agent::{AgentDeps, AgentLoop};
use crate::context::ContextBuilder;
use crate::init::MachineAudit;
use crate::input::{classify, InputKind};
use crate::model_cmd::{handle_model_command, ModelCommandContext, ModelListCache};
use crate::raw_shell::{resolve_shell, RiskScan};
use llmsh_audit::event::{now_iso, AuditEvent};
use reedline::{Reedline, Signal};
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
    pub builder: ContextBuilder,
    pub raw_shell: Option<String>,
    pub risk_scan: RiskScan,
    pub root_cancel: CancellationToken,
    pub config_path: Option<PathBuf>,
    pub model_cache: ModelListCache,
    pub model_provider_prefix: Option<String>,
    pub prompt: Box<dyn reedline::Prompt>,
}

impl Repl {
    pub async fn run(mut self) -> anyhow::Result<()> {
        let mut line_editor = Reedline::create();
        loop {
            match line_editor.read_line(self.prompt.as_ref())? {
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
                                builder: std::mem::replace(
                                    &mut self.builder,
                                    ContextBuilder::new(0),
                                ),
                            };
                            let result = loop_state.run(&t).await;
                            // restore the builder (now mutated with this turn's
                            // user/assistant/tool messages) back into the repl.
                            self.builder = loop_state.builder;
                            match result {
                                Ok(r) => {
                                    if let Some(text) = r.assistant_text {
                                        println!("{}", text);
                                    }
                                    let level = self.deps.verbose;
                                    if level > 0 {
                                        if let Ok(s) = self.deps.stats.read() {
                                            crate::verbose_print::print_turn_verbose(
                                                &mut std::io::stderr(),
                                                &s,
                                                level,
                                            );
                                        }
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
                println!("/help /exit /clear-context /clear-memory /clear-all /compact /memory [list|forget <id>|add [category:]<claim>] /pwd /cd <path> /history /model [list|set <id>] /init");
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
                    model_provider_prefix: self.model_provider_prefix.clone(),
                };
                if let Err(e) = handle_model_command(&ctx, args).await {
                    eprintln!("model command error: {}", e);
                }
            }
            "clear" => {
                println!("(/clear is deprecated, use /clear-context — applying it now)");
                self.do_clear_context().await;
            }
            "clear-context" => self.do_clear_context().await,
            "clear-memory" => self.do_clear_memory().await,
            "clear-all" => {
                self.do_clear_context().await;
                self.do_clear_memory().await;
            }
            "compact" => {
                let model_now = self
                    .deps
                    .model_label
                    .read()
                    .map(|g| g.clone())
                    .unwrap_or_else(|_| "unknown".into());
                let last_input = self
                    .deps
                    .stats
                    .read()
                    .ok()
                    .and_then(|s| s.last_turn.as_ref().map(|t| t.input_tokens))
                    .unwrap_or(0);
                let report = crate::compactor::compact(
                    &mut self.builder.messages,
                    &self.deps.compact_config,
                    &self.deps.memory_cfg,
                    crate::compactor::CompactionReason::Manual,
                    &model_now,
                    last_input.max(u32::MAX / 2),
                    self.deps.provider.clone(),
                    self.deps.memory.clone(),
                )
                .await;
                println!(
                    "compacted: {} → {} messages, {} → {} bytes ({})",
                    report.messages_before,
                    report.messages_after,
                    report.bytes_before,
                    report.bytes_after,
                    report.strategy.as_str(),
                );
                let _ = self
                    .deps
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
            "memory" => self.handle_memory_subcommand(args).await,
            other => eprintln!("unknown meta command: /{}", other),
        }
        Ok(())
    }

    async fn do_clear_context(&mut self) {
        let ts = now_iso();
        let n = match self
            .deps
            .memory
            .mark_conversation_cleared(&ts, crate::memory::ClearSource::ClearContext)
        {
            Ok(n) => n,
            Err(e) => {
                eprintln!("clear-context: SQLite error: {e}");
                0
            }
        };
        self.builder.messages.clear();
        println!("clear-context: cleared {n} message(s)");
        let _ = self
            .deps
            .audit
            .lock()
            .unwrap()
            .write(&AuditEvent::ContextCleared {
                ts,
                scope: "context".into(),
                rows_affected: n,
            });
    }

    async fn do_clear_memory(&mut self) {
        let ts = now_iso();
        let n = match self
            .deps
            .memory
            .mark_facts_cleared(&ts, crate::memory::ClearSource::ClearMemory)
        {
            Ok(n) => n,
            Err(e) => {
                eprintln!("clear-memory: SQLite error: {e}");
                0
            }
        };
        println!("clear-memory: dropped {n} long-term fact(s)");
        let _ = self
            .deps
            .audit
            .lock()
            .unwrap()
            .write(&AuditEvent::ContextCleared {
                ts,
                scope: "memory".into(),
                rows_affected: n,
            });
    }

    async fn handle_memory_subcommand(&mut self, args: &[String]) {
        let sub = args.first().map(String::as_str).unwrap_or("list");
        match sub {
            "list" => {
                let facts = match self.deps.memory.load_active_facts() {
                    Ok(f) => f,
                    Err(e) => {
                        eprintln!("memory list error: {e}");
                        return;
                    }
                };
                if facts.is_empty() {
                    println!("(no long-term facts yet)");
                    return;
                }
                for f in facts {
                    println!(
                        "[{}] #{} ({}): {}",
                        f.category, f.id, f.insert_source, f.claim
                    );
                }
            }
            "forget" => {
                let Some(id_str) = args.get(1) else {
                    eprintln!("usage: /memory forget <id>");
                    return;
                };
                let id: i64 = match id_str.parse() {
                    Ok(n) => n,
                    Err(_) => {
                        eprintln!("invalid id: {id_str}");
                        return;
                    }
                };
                let ts = now_iso();
                let removed = match self.deps.memory.mark_fact_cleared_by_id(
                    id,
                    &ts,
                    crate::memory::ClearSource::MemoryForget,
                ) {
                    Ok(b) => b,
                    Err(e) => {
                        eprintln!("memory forget error: {e}");
                        return;
                    }
                };
                if removed {
                    println!("forgot fact #{id}");
                    let _ = self
                        .deps
                        .audit
                        .lock()
                        .unwrap()
                        .write(&AuditEvent::ContextCleared {
                            ts,
                            scope: "memory_forget".into(),
                            rows_affected: 1,
                        });
                } else {
                    println!("fact #{id} not found or already cleared");
                }
            }
            "add" => {
                let claim_parts: Vec<String> = args.iter().skip(1).cloned().collect();
                if claim_parts.is_empty() {
                    eprintln!("usage: /memory add <claim text>");
                    return;
                }
                let raw = claim_parts.join(" ");
                let (category, claim) = match raw.split_once(':') {
                    Some((cat, rest))
                        if matches!(
                            cat.trim(),
                            "identity" | "preference" | "project" | "todo" | "other"
                        ) =>
                    {
                        (cat.trim().to_string(), rest.trim().to_string())
                    }
                    _ => ("other".to_string(), raw),
                };
                let ts = now_iso();
                let id = match self.deps.memory.add_manual_fact(&ts, &category, &claim) {
                    Ok(id) => id,
                    Err(e) => {
                        eprintln!("memory add error: {e}");
                        return;
                    }
                };
                println!("added fact #{id} [{category}]: {claim}");
                let _ = self
                    .deps
                    .audit
                    .lock()
                    .unwrap()
                    .write(&AuditEvent::FactAdded {
                        ts,
                        fact_id: id,
                        category,
                        source: "manual".into(),
                    });
            }
            other => eprintln!("unknown /memory subcommand: {other} (use list, forget, add)"),
        }
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
