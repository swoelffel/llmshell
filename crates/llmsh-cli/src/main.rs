use anyhow::Context as _;
use clap::Parser;
use llmsh_audit::event::{now_iso, AuditEvent};
use llmsh_audit::redact::Redactor;
use llmsh_audit::session::new_session_id;
use llmsh_audit::writer::AuditWriter;
use llmsh_core::agent::{AgentBounds, AgentDeps};
use llmsh_core::agents_md::load_agents_md;
use llmsh_core::config::load::{load_or_create_user, load_project, user_config_path};
use llmsh_core::config::merge::merge_project;
use llmsh_core::config::Config;
use llmsh_core::confirm::StdinConfirmationGate;
use llmsh_core::context::MemorySystemPrompt;
use llmsh_core::executor::ToolExecutor;
use llmsh_core::init::run_autoinit_if_needed;
use llmsh_core::memory::Memory;
use llmsh_core::model_cmd::ModelListCache;
use llmsh_core::pipeline::Pipeline;
use llmsh_core::raw_shell::RiskScan;
use llmsh_core::repl::{Repl, ReplState};
use llmsh_core::setup::{
    finalize_setup, load_existing_or_default_config, run_setup_flow, SetupOutcome, SetupPrompts,
    SetupProvider,
};
use llmsh_llm::provider::LlmProvider;
use llmsh_llm_anthropic::provider::{
    AnthropicConfig, AnthropicProvider, DEFAULT_MAX_TOKENS as ANTHROPIC_DEFAULT_MAX_TOKENS,
};
use llmsh_llm_mistral::provider::{MistralConfig, MistralProvider};
use llmsh_llm_ollama::provider::{OllamaConfig, OllamaProvider};
use llmsh_llm_openai::provider::{OpenAIConfig, OpenAIProvider};
use llmsh_policy::context::PolicyContext;
use llmsh_policy::engine::{DefaultPolicyConfig, DefaultPolicyEngine, PolicyEngine, RiskAction};
use llmsh_policy::types::RiskLevel;
use llmsh_tools::list_directory::ListDirectory;
use llmsh_tools::read_file::ReadFile;
use llmsh_tools::registry::ToolRegistry;
use llmsh_tools::run_process::RunProcess;
use std::io::IsTerminal as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

type ProviderWithModel = (Arc<dyn LlmProvider>, Arc<RwLock<String>>, Option<String>);
use tokio_util::sync::CancellationToken;

#[derive(Parser, Debug)]
#[command(version, about = "LLMShell — agentic shell")]
struct Cli {
    #[arg(long, env = "LLMSH_CONFIG")]
    config: Option<PathBuf>,
    #[arg(long, env = "LLMSH_MODEL")]
    model: Option<String>,
    /// Verbose output: -v = tier 1, -vv = tier 1 + tier 2.
    #[arg(short = 'v', long, action = clap::ArgAction::Count)]
    verbose: u8,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MissingProviderKey {
    provider: String,
    env_var: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FirstLaunchAction {
    Continue,
    RunSetup,
    PrintManualInstructionsAndExit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum MissingKeyRecoveryAction {
    RetryAfterSetup(MissingProviderKey),
    ReturnErrorWithSetupHint,
    ReturnOriginalError,
}

#[derive(clap::Subcommand, Debug)]
enum Cmd {
    /// Run first-time provider/API key/model setup.
    Setup,
    /// Verify the hash chain of an audit log.
    VerifyAudit {
        /// Path to the .jsonl audit file.
        path: PathBuf,
        /// Session id used to seed the chain. Defaults to the file stem.
        #[arg(long)]
        session_id: Option<String>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let session_start = std::time::Instant::now();
    if std::env::var("LLMSH_DEBUG").ok().as_deref() == Some("1") {
        tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .init();
    }
    let cli = Cli::parse();

    match cli.cmd {
        Some(Cmd::Setup) => {
            let cfg_path = user_config_path(cli.config.as_deref())
                .ok_or_else(|| anyhow::anyhow!("could not determine config dir"))?;
            return run_interactive_setup(&cfg_path).await;
        }
        Some(Cmd::VerifyAudit { path, session_id }) => {
            let sid = session_id.unwrap_or_else(|| {
                path.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string()
            });
            let jsonl = std::fs::read_to_string(&path)
                .with_context(|| format!("read {}", path.display()))?;
            match llmsh_audit::verify_chain(&jsonl, &sid) {
                Ok(v) if v.sealed => {
                    println!("OK: {} events, sealed (session_ended present).", v.events);
                    return Ok(());
                }
                Ok(v) => {
                    println!(
                        "OK (unsealed): {} events, no session_ended — file is internally consistent but may have been truncated or the writer crashed.",
                        v.events
                    );
                    return Ok(());
                }
                Err(e) => {
                    eprintln!("FAIL: {e}");
                    std::process::exit(2);
                }
            }
        }
        None => {}
    }

    // 1. Load config
    let cfg_path = user_config_path(cli.config.as_deref())
        .ok_or_else(|| anyhow::anyhow!("could not determine config dir"))?;
    let workspace_root = std::env::current_dir()?;
    let (mut cfg, created) =
        load_config_with_overrides(&cfg_path, cli.model.as_deref(), &workspace_root)?;
    if created {
        println!("No llmsh config found. Created {}.", cfg_path.display());
        let interactive = stdin_is_interactive();
        let action = decide_first_launch_action(
            created,
            interactive,
            interactive && prompt_run_setup_now()?,
        );
        match action {
            FirstLaunchAction::Continue => {}
            FirstLaunchAction::RunSetup => {
                run_interactive_setup(&cfg_path).await?;
                cfg =
                    load_config_with_overrides(&cfg_path, cli.model.as_deref(), &workspace_root)?.0;
            }
            FirstLaunchAction::PrintManualInstructionsAndExit => {
                print_manual_setup_instructions();
                return Ok(());
            }
        }
    }

    let verbose_level: u8 = if cli.verbose > 0 {
        cli.verbose.min(2)
    } else if let Ok(s) = std::env::var("LLMSH_VERBOSE") {
        s.trim().parse::<u8>().unwrap_or(0).min(2)
    } else {
        cfg.verbose.default_level.min(2)
    };

    // 2. Provider
    // Chain: inner concrete provider → SwappableProvider (hot-swap shim) →
    // ThinkingProvider (status indicator). The SwappableProvider handle is
    // kept in the REPL so `/provider set …` can replace the inner provider.
    let (inner, shared_model, provider_prefix) = match build_provider(&cfg) {
        Ok(provider) => provider,
        Err(err) => {
            let interactive = stdin_is_interactive();
            let action = match cfg.default_model.split_once(':') {
                Some((provider_name, _)) => {
                    if let Some(missing) = missing_provider_key_from_error(provider_name, &err) {
                        let run_setup_now = if interactive {
                            eprintln!(
                                "Missing {} for provider {}. Run setup now? [Y/n]",
                                missing.env_var, missing.provider
                            );
                            prompt_yes_no_default_yes()?
                        } else {
                            false
                        };
                        decide_missing_key_recovery_action(
                            &cfg.default_model,
                            &err,
                            interactive,
                            run_setup_now,
                        )
                    } else {
                        MissingKeyRecoveryAction::ReturnOriginalError
                    }
                }
                None => MissingKeyRecoveryAction::ReturnOriginalError,
            };
            match action {
                MissingKeyRecoveryAction::RetryAfterSetup(_) => {
                    run_interactive_setup(&cfg_path).await?;
                    cfg = load_config_with_overrides(
                        &cfg_path,
                        cli.model.as_deref(),
                        &workspace_root,
                    )?
                    .0;
                    build_provider(&cfg)?
                }
                MissingKeyRecoveryAction::ReturnErrorWithSetupHint => {
                    return Err(err)
                        .context("run `llmsh setup` to configure a provider and API key");
                }
                MissingKeyRecoveryAction::ReturnOriginalError => {
                    return Err(err);
                }
            }
        }
    };
    let swappable = Arc::new(llmsh_core::swappable::SwappableProvider::new(
        inner,
        shared_model.clone(),
    ));
    let provider: Arc<dyn LlmProvider> = Arc::new(llmsh_core::thinking::ThinkingProvider::new(
        swappable.clone() as Arc<dyn LlmProvider>,
    ));

    // 3. Audit
    let no_audit = std::env::var("LLMSH_NO_AUDIT").ok().as_deref() == Some("1");
    if no_audit {
        eprintln!("⚠ LLMSH_NO_AUDIT=1: audit disabled, /history will be empty.");
    }
    let session_id = new_session_id();
    let audit_dir = expand_tilde(&cfg.audit.directory);
    let mut writer = if no_audit {
        AuditWriter::disabled()
    } else {
        AuditWriter::open(&audit_dir, &session_id)?
    };
    // Emit SessionStarted only after any first-run setup / missing-key recovery
    // has finished so the audit record reflects the final effective config.
    writer.write(&session_started_event(
        session_id.clone(),
        &workspace_root,
        &cfg,
    ))?;

    // 4. Tools
    let mut registry = ToolRegistry::new();
    registry.register(Arc::new(ListDirectory));
    registry.register(Arc::new(ReadFile));
    registry.register(Arc::new(RunProcess));
    registry.register(Arc::new(llmsh_tools::glob::Glob));
    let registry = Arc::new(registry);

    // 5. Policy
    let policy =
        Arc::new(DefaultPolicyEngine::new(policy_config_from(&cfg))) as Arc<dyn PolicyEngine>;
    let allowed_roots: Vec<PathBuf> = cfg
        .policy
        .filesystem
        .allowed_roots
        .iter()
        .map(|r| {
            if r == "." {
                workspace_root.clone()
            } else {
                PathBuf::from(r)
            }
        })
        .collect();
    let shared_cwd = llmsh_core::cwd::new_shared(workspace_root.clone());
    let oldpwd = Arc::new(std::sync::Mutex::new(None::<PathBuf>));
    let policy_ctx = PolicyContext {
        cwd: shared_cwd.clone(),
        workspace_root: workspace_root.clone(),
        allowed_roots: allowed_roots.clone(),
        sensitive_path_patterns: cfg.policy.sensitive_paths.patterns.clone(),
    };

    let home = directories::UserDirs::new().map(|d| d.home_dir().to_path_buf());
    let pipeline = Pipeline {
        registry: registry.clone(),
        policy: policy.clone(),
        home: home.clone(),
        auto_classify_run_process: cfg.policy.run_process.auto_classify_read_only,
    };

    let cancel = CancellationToken::new();
    let executor = ToolExecutor {
        registry: registry.clone(),
        timeout: std::time::Duration::from_millis(cfg.limits.tool_timeout_ms),
        max_output_bytes: cfg.limits.max_audit_output_bytes,
        env: filtered_env(),
        cancel: cancel.clone(),
        home: home.clone(),
    };

    // 6. Memory
    let memory_path = memory_path_from_env_or_default(std::env::var("LLMSH_MEMORY_DB").ok())?;
    let memory = Arc::new(Memory::open(&memory_path)?);

    // One-shot cleanup of orphan assistant.tool_calls left over from a prior
    // session that ended on Deny / Cancel before v0.2.7. Without this OpenAI
    // returns 400 on the next reload.
    match memory.cleanup_orphan_tool_calls(&now_iso()) {
        Ok(0) => {}
        Ok(n) => println!("(cleaned {} orphan tool call(s) from prior session)", n),
        Err(e) => tracing::warn!("orphan cleanup failed: {}", e),
    }

    // Auto-bootstrap: run /init on first launch when DB has no audit entry.
    let no_autoinit = std::env::var("LLMSH_NO_AUTOINIT")
        .ok()
        .filter(|v| !v.is_empty())
        .is_some();
    if run_autoinit_if_needed(&memory, no_autoinit).await? {
        println!("(initial machine audit — type /init to refresh)");
    }

    // 7. Load active conversation from SQLite (if enabled)
    let initial_messages: Vec<llmsh_llm::types::Message> = if cfg.memory.auto_load_conversation {
        match memory.load_active_conversation() {
            Ok(rows) => rows
                .into_iter()
                .map(|r| llmsh_llm::types::Message {
                    role: match r.role.as_str() {
                        "user" => llmsh_llm::types::MessageRole::User,
                        "assistant" => llmsh_llm::types::MessageRole::Assistant,
                        "tool" => llmsh_llm::types::MessageRole::Tool,
                        "system" => llmsh_llm::types::MessageRole::System,
                        _ => llmsh_llm::types::MessageRole::User,
                    },
                    content: r.content,
                    tool_call_id: r.tool_call_id,
                    name: r.name,
                    tool_calls: r
                        .tool_calls_json
                        .and_then(|s| serde_json::from_str(&s).ok()),
                })
                .collect(),
            Err(e) => {
                tracing::warn!("load_active_conversation failed, starting empty: {}", e);
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    let initial_count = initial_messages.len();
    if initial_count > 0 {
        println!(
            "(reloaded {} messages from previous session)",
            initial_count
        );
    }

    // 8. Agent deps
    let agents_md = load_agents_md();
    let system_prompt = Arc::new(MemorySystemPrompt::new(
        agents_md,
        memory.clone(),
        workspace_root.clone(),
        shared_model.clone(),
        session_start,
    ));
    let stats = Arc::new(RwLock::new(
        llmsh_core::session_stats::SessionStats::default(),
    ));
    let deps = Arc::new(AgentDeps {
        provider,
        pipeline,
        executor,
        gate: Arc::new(StdinConfirmationGate),
        audit: std::sync::Mutex::new(writer),
        redactor: Redactor::default_audit(),
        bounds: AgentBounds {
            max_iterations: cfg.limits.max_iterations,
            max_tool_calls_per_iteration: cfg.limits.max_tool_calls_per_iteration,
            max_schema_repair_attempts: cfg.limits.max_schema_repair_attempts,
        },
        compact_config: cfg.compact.clone(),
        memory_cfg: cfg.memory.clone(),
        policy_ctx,
        sensitive_patterns: cfg.policy.sensitive_paths.patterns.clone(),
        model_label: shared_model.clone(),
        system_prompt,
        memory,
        verbose: verbose_level,
        stats: stats.clone(),
        oldpwd: oldpwd.clone(),
        home: home.clone(),
    });

    let prompt: Box<dyn reedline::Prompt> = if cfg.verbose.status_line {
        Box::new(llmsh_core::status_prompt::StatusPrompt::new(
            shared_model.clone(),
            stats.clone(),
            true,
        ))
    } else {
        Box::new(reedline::DefaultPrompt::default())
    };

    let repl = Repl {
        deps,
        state: ReplState {
            cwd: shared_cwd.clone(),
            workspace_root,
            allowed_roots,
            history_recent: vec![],
        },
        builder: llmsh_core::context::ContextBuilder::with_messages(
            cfg.limits.max_llm_output_bytes,
            initial_messages,
        ),
        raw_shell: cfg.shell.raw_shell.clone(),
        risk_scan: RiskScan::default(),
        root_cancel: cancel,
        config_path: Some(cfg_path),
        model_cache: ModelListCache::new(),
        model_provider_prefix: provider_prefix,
        prompt,
        cfg: cfg.clone(),
        swappable,
        provider_swapper: Box::new(CliProviderSwapper),
    };
    repl.run().await?;
    Ok(())
}

struct StdinSetupPrompts;

impl SetupPrompts for StdinSetupPrompts {
    fn choose_provider(&mut self, providers: &[SetupProvider]) -> anyhow::Result<Option<String>> {
        println!("Select a provider:");
        for (idx, provider) in providers.iter().enumerate() {
            println!("  {}. {}", idx + 1, provider.display_name);
        }
        read_index_selection("Provider", providers.len())
            .map(|idx| Some(providers[idx].name.clone()))
    }

    fn read_api_key(&mut self, provider: &SetupProvider) -> anyhow::Result<Option<String>> {
        let env_var = provider.api_key_env.as_deref().unwrap_or("API_KEY");
        println!("Enter {env_var} for {}:", provider.display_name);
        let value = read_secret_line("> ")?;
        if value.is_empty() {
            return Ok(None);
        }
        Ok(Some(value))
    }

    fn choose_model(
        &mut self,
        provider: &str,
        models: &[String],
    ) -> anyhow::Result<Option<String>> {
        println!("Select a model for {provider}:");
        for (idx, model) in models.iter().enumerate() {
            println!("  {}. {}", idx + 1, model);
        }
        read_index_selection("Model", models.len()).map(|idx| Some(models[idx].clone()))
    }

    fn confirm_persist_env(&mut self, profile: &Path, env_var: &str) -> anyhow::Result<bool> {
        println!(
            "Append {} to {} so future shells inherit it? [Y/n]",
            env_var,
            profile.display()
        );
        let answer = read_trimmed_line("> ")?;
        Ok(!matches!(answer.to_ascii_lowercase().as_str(), "n" | "no"))
    }
}

async fn run_interactive_setup(config_path: &Path) -> anyhow::Result<()> {
    run_interactive_setup_with(
        config_path,
        &mut StdinSetupPrompts,
        |key, value| std::env::set_var(key, value),
        validate_setup_choice,
        confirm_finish_anyway,
    )
    .await
}

enum SetupValidationError {
    Local(anyhow::Error),
    Network(anyhow::Error),
}

async fn run_interactive_setup_with<P, E, V, C, Fut>(
    config_path: &Path,
    prompts: &mut P,
    env_setter: E,
    validate: V,
    mut confirm_finish_anyway: C,
) -> anyhow::Result<()>
where
    P: SetupPrompts,
    E: FnMut(&str, &str),
    V: Fn(PathBuf, SetupOutcome) -> Fut,
    C: FnMut() -> anyhow::Result<bool>,
    Fut: std::future::Future<Output = Result<(), SetupValidationError>>,
{
    let outcome = run_setup_flow(config_path, prompts, env_setter)?;

    match validate(config_path.to_path_buf(), outcome.clone()).await {
        Ok(()) => {}
        Err(SetupValidationError::Local(err)) => {
            anyhow::bail!("provider validation failed before setup could be saved: {err}");
        }
        Err(SetupValidationError::Network(err)) => {
            eprintln!("Provider validation failed: {err}");
            if !confirm_finish_anyway()? {
                anyhow::bail!("setup aborted after provider validation failure");
            }
        }
    }

    let profile_updated = finalize_setup(config_path, &outcome)?;
    println!(
        "Saved default model {}:{} in {}.",
        outcome.provider,
        outcome.model,
        config_path.display()
    );
    if let Some(profile) = profile_updated {
        println!("Updated {}.", profile.display());
    }
    Ok(())
}

async fn validate_setup_choice(
    config_path: PathBuf,
    outcome: SetupOutcome,
) -> Result<(), SetupValidationError> {
    let cfg = load_existing_or_default_config(&config_path).map_err(SetupValidationError::Local)?;
    let provider = build_inner_provider(&outcome.provider, &outcome.model, &cfg)
        .map_err(SetupValidationError::Local)?;
    provider.list_models().await.map(|_| ()).map_err(|err| {
        let err = anyhow::Error::from(err);
        if is_network_validation_error(&err) {
            SetupValidationError::Network(err)
        } else {
            SetupValidationError::Local(err)
        }
    })
}

fn load_config_with_overrides(
    cfg_path: &Path,
    cli_model: Option<&str>,
    workspace_root: &Path,
) -> anyhow::Result<(Config, bool)> {
    let (mut cfg, created) = load_or_create_user(cfg_path)?;
    if let Some(model) = cli_model {
        cfg.default_model = model.to_string();
    }
    if let Some(project) = load_project(workspace_root)? {
        let _ = merge_project(&mut cfg, &project);
    }
    Ok((cfg, created))
}

fn is_network_validation_error(err: &anyhow::Error) -> bool {
    const NETWORK_PATTERNS: &[&str] = &[
        "connection refused",
        "failed to connect",
        "dns error",
        "name or service not known",
        "network unreachable",
        "operation timed out",
        "timed out",
        "reqwest connect error",
    ];

    err.chain().any(|cause| {
        let message = cause.to_string().to_ascii_lowercase();
        NETWORK_PATTERNS
            .iter()
            .any(|pattern| message.contains(pattern))
    })
}

fn missing_provider_key_from_error(
    provider: &str,
    err: &anyhow::Error,
) -> Option<MissingProviderKey> {
    let msg = format!("{:#}", err);
    let prefix = "env var ";
    let suffix = " not set";
    let start = msg.find(prefix)? + prefix.len();
    let rest = &msg[start..];
    let end = rest.find(suffix)?;
    Some(MissingProviderKey {
        provider: provider.to_string(),
        env_var: rest[..end].to_string(),
    })
}

fn decide_first_launch_action(
    created: bool,
    interactive: bool,
    run_setup_now: bool,
) -> FirstLaunchAction {
    if !created {
        return FirstLaunchAction::Continue;
    }

    if interactive && run_setup_now {
        FirstLaunchAction::RunSetup
    } else {
        FirstLaunchAction::PrintManualInstructionsAndExit
    }
}

fn decide_missing_key_recovery_action(
    default_model: &str,
    err: &anyhow::Error,
    interactive: bool,
    run_setup_now: bool,
) -> MissingKeyRecoveryAction {
    let Some((provider_name, _)) = default_model.split_once(':') else {
        return MissingKeyRecoveryAction::ReturnOriginalError;
    };
    let Some(missing) = missing_provider_key_from_error(provider_name, err) else {
        return MissingKeyRecoveryAction::ReturnOriginalError;
    };

    if interactive && run_setup_now {
        MissingKeyRecoveryAction::RetryAfterSetup(missing)
    } else {
        MissingKeyRecoveryAction::ReturnErrorWithSetupHint
    }
}

fn session_started_event(session_id: String, workspace_root: &Path, cfg: &Config) -> AuditEvent {
    AuditEvent::SessionStarted {
        ts: now_iso(),
        session_id,
        cwd: workspace_root.display().to_string(),
        model: cfg.default_model.clone(),
        policy_mode: cfg.policy.unknown.clone(),
        llmsh_version: env!("CARGO_PKG_VERSION").into(),
        schema_version: llmsh_audit::event::SCHEMA_VERSION,
        config_effective_hash: cfg.effective_hash(),
    }
}

fn build_provider(cfg: &Config) -> anyhow::Result<ProviderWithModel> {
    let (provider_name, model) = cfg
        .default_model
        .split_once(':')
        .ok_or_else(|| anyhow::anyhow!("default_model must be \"provider:model\""))?;
    let inner = build_inner_provider(provider_name, model, cfg)?;
    let shared = Arc::new(RwLock::new(model.to_string()));
    Ok((inner, shared, Some(provider_name.to_string())))
}

/// Concrete provider factory: dispatches on provider name. Used both at
/// startup and (via `CliProviderSwapper`) for `/provider set <name>`.
fn build_inner_provider(
    provider_name: &str,
    model: &str,
    cfg: &Config,
) -> anyhow::Result<Arc<dyn LlmProvider>> {
    let pcfg = cfg
        .providers
        .get(provider_name)
        .ok_or_else(|| anyhow::anyhow!("provider {} not configured", provider_name))?;
    match provider_name {
        "openai" => {
            let env_var = pcfg
                .api_key_env
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("openai provider requires api_key_env"))?;
            let api_key = std::env::var(env_var)
                .map_err(|_| anyhow::anyhow!("env var {} not set", env_var))?;
            let p = OpenAIProvider::new(OpenAIConfig {
                base_url: pcfg.base_url.clone(),
                api_key,
                model: model.into(),
                timeout_ms: 60_000,
            })?;
            Ok(Arc::new(p))
        }
        "anthropic" => {
            let env_var = pcfg
                .api_key_env
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("anthropic provider requires api_key_env"))?;
            let api_key = std::env::var(env_var)
                .map_err(|_| anyhow::anyhow!("env var {} not set", env_var))?;
            let p = AnthropicProvider::new(AnthropicConfig {
                base_url: pcfg.base_url.clone(),
                api_key,
                model: model.into(),
                timeout_ms: 60_000,
                max_tokens: ANTHROPIC_DEFAULT_MAX_TOKENS,
            })?;
            Ok(Arc::new(p))
        }
        "ollama" => {
            // Ollama default config does not require auth (local server). If
            // the user did set api_key_env we ignore it for now.
            let p = OllamaProvider::new(OllamaConfig {
                base_url: pcfg.base_url.clone(),
                model: model.into(),
                timeout_ms: 120_000,
            })?;
            Ok(Arc::new(p))
        }
        "mistral" => {
            let env_var = pcfg
                .api_key_env
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("mistral provider requires api_key_env"))?;
            let api_key = std::env::var(env_var)
                .map_err(|_| anyhow::anyhow!("env var {} not set", env_var))?;
            let p = MistralProvider::new(MistralConfig {
                base_url: pcfg.base_url.clone(),
                api_key,
                model: model.into(),
                timeout_ms: 60_000,
            })?;
            Ok(Arc::new(p))
        }
        other => anyhow::bail!(
            "unknown provider \"{}\"; supported: openai, anthropic, ollama, mistral",
            other
        ),
    }
}

struct CliProviderSwapper;

impl llmsh_core::provider_cmd::ProviderSwapper for CliProviderSwapper {
    fn build(&self, name: &str, model: &str, cfg: &Config) -> anyhow::Result<Arc<dyn LlmProvider>> {
        build_inner_provider(name, model, cfg)
    }
}

fn policy_config_from(cfg: &Config) -> DefaultPolicyConfig {
    let map_action = |s: &str| match s {
        "allow" => RiskAction::Allow,
        "confirm" => RiskAction::Confirm,
        "confirm_strong" => RiskAction::ConfirmStrong,
        "deny" => RiskAction::Deny,
        _ => RiskAction::Confirm,
    };
    let mut m = std::collections::HashMap::new();
    m.insert(RiskLevel::ReadOnly, map_action(&cfg.policy.read_only));
    m.insert(RiskLevel::LowRisk, map_action(&cfg.policy.low_risk));
    m.insert(RiskLevel::Write, map_action(&cfg.policy.write));
    m.insert(RiskLevel::Destructive, map_action(&cfg.policy.destructive));
    m.insert(RiskLevel::Network, map_action(&cfg.policy.network));
    m.insert(RiskLevel::Privileged, map_action(&cfg.policy.privileged));
    m.insert(RiskLevel::Unknown, map_action(&cfg.policy.unknown));
    DefaultPolicyConfig { risk_actions: m }
}

fn expand_tilde(s: &str) -> PathBuf {
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = directories::UserDirs::new().map(|d| d.home_dir().to_path_buf()) {
            return home.join(rest);
        }
    }
    PathBuf::from(s)
}

fn filtered_env() -> std::collections::HashMap<String, String> {
    let allow = ["PATH", "HOME", "USER", "LANG", "LC_ALL", "TERM", "SHELL"];
    std::env::vars()
        .filter(|(k, _)| allow.contains(&k.as_str()))
        .collect()
}

fn memory_path_from_env_or_default(env_value: Option<String>) -> anyhow::Result<PathBuf> {
    if let Some(v) = env_value.filter(|s| !s.is_empty()) {
        return Ok(PathBuf::from(v));
    }
    directories::ProjectDirs::from("", "", "llmsh")
        .map(|d| d.data_dir().join("memory.db"))
        .ok_or_else(|| anyhow::anyhow!("could not determine data dir for memory.db"))
}

fn read_trimmed_line(prompt: &str) -> anyhow::Result<String> {
    use std::io::Write as _;

    print!("{prompt}");
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    Ok(line.trim().to_string())
}

fn read_secret_line(prompt: &str) -> anyhow::Result<String> {
    read_secret_line_with(prompt, || rpassword::read_password())
}

fn read_secret_line_with(
    prompt: &str,
    read_secret: impl FnOnce() -> std::io::Result<String>,
) -> anyhow::Result<String> {
    use std::io::Write as _;

    print!("{prompt}");
    std::io::stdout().flush()?;
    let line = read_secret()?;
    Ok(line.trim().to_string())
}

fn read_index_selection(label: &str, len: usize) -> anyhow::Result<usize> {
    let prompt = format!("{label} [1]: ");
    let input = read_trimmed_line(&prompt)?;
    if input.is_empty() {
        return Ok(0);
    }
    let selected: usize = input.parse()?;
    if !(1..=len).contains(&selected) {
        anyhow::bail!("{label} selection must be between 1 and {len}");
    }
    Ok(selected - 1)
}

fn stdin_is_interactive() -> bool {
    std::io::stdin().is_terminal()
}

fn prompt_run_setup_now() -> anyhow::Result<bool> {
    println!("Run setup now? [Y/n]");
    prompt_yes_no_default_yes()
}

fn prompt_yes_no_default_yes() -> anyhow::Result<bool> {
    let answer = read_trimmed_line("> ")?;
    Ok(!matches!(answer.to_ascii_lowercase().as_str(), "n" | "no"))
}

fn print_manual_setup_instructions() {
    println!();
    println!("Set one provider API key, then run `llmsh setup` or `llmsh` again:");
    println!("  export OPENAI_API_KEY=...");
    println!("  export ANTHROPIC_API_KEY=...");
    println!("  export MISTRAL_API_KEY=...");
}

fn confirm_finish_anyway() -> anyhow::Result<bool> {
    println!("Finish setup anyway? [y/N]");
    let answer = read_trimmed_line("> ")?;
    Ok(matches!(answer.to_ascii_lowercase().as_str(), "y" | "yes"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use std::path::Path;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    struct FakePrompts {
        provider: Option<String>,
        api_key: Option<String>,
        model: Option<String>,
        persist_env: bool,
    }

    impl SetupPrompts for FakePrompts {
        fn choose_provider(
            &mut self,
            _providers: &[SetupProvider],
        ) -> anyhow::Result<Option<String>> {
            Ok(self.provider.take())
        }

        fn read_api_key(&mut self, _provider: &SetupProvider) -> anyhow::Result<Option<String>> {
            Ok(self.api_key.take())
        }

        fn choose_model(
            &mut self,
            _provider: &str,
            _models: &[String],
        ) -> anyhow::Result<Option<String>> {
            Ok(self.model.take())
        }

        fn confirm_persist_env(&mut self, _profile: &Path, _env_var: &str) -> anyhow::Result<bool> {
            Ok(self.persist_env)
        }
    }

    #[test]
    fn parses_setup_subcommand() {
        let cli = Cli::try_parse_from(["llmsh", "setup"]).unwrap();
        assert!(matches!(cli.cmd, Some(Cmd::Setup)));
    }

    #[test]
    fn empty_env_falls_back_to_default() {
        let p = memory_path_from_env_or_default(Some("".into())).unwrap();
        assert!(p.ends_with("memory.db"));
        assert!(p.parent().is_some());
    }

    #[test]
    fn unset_env_falls_back_to_default() {
        let p = memory_path_from_env_or_default(None).unwrap();
        assert!(p.ends_with("memory.db"));
    }

    #[test]
    fn explicit_env_used_verbatim() {
        let p = memory_path_from_env_or_default(Some("/tmp/custom.db".into())).unwrap();
        assert_eq!(p, PathBuf::from("/tmp/custom.db"));
    }

    #[test]
    fn build_inner_provider_supports_mistral() {
        std::env::set_var(
            "MISTRAL_API_KEY",
            "mistral-api-key-EXAMPLE_FIXTURE_NOT_REAL",
        );
        let mut cfg = Config::defaults();
        cfg.providers.insert(
            "mistral".into(),
            llmsh_core::config::ProviderConfig {
                api_key_env: Some("MISTRAL_API_KEY".into()),
                base_url: "https://api.mistral.ai/v1".into(),
                tool_calling: "native".into(),
                models: vec!["mistral-medium-3-5".into()],
            },
        );
        let provider = build_inner_provider("mistral", "mistral-medium-3-5", &cfg)
            .expect("mistral provider should build");
        assert_eq!(provider.current_model(), "mistral-medium-3-5");
    }

    #[test]
    fn secret_reader_trims_without_using_stdin_lines() {
        let value = read_secret_line_with("> ", || Ok("  sk-test-secret  \n".to_string())).unwrap();
        assert_eq!(value, "sk-test-secret");
    }

    #[test]
    fn network_validation_error_detection_is_conservative() {
        for network_message in [
            "error sending request for url (https://api.openai.com/v1/models): connection refused",
            "dns error: failed to lookup address information: Name or service not known",
            "operation timed out",
            "network unreachable",
            "request failed: timed out",
            "reqwest connect error",
        ] {
            assert!(
                is_network_validation_error(&anyhow::anyhow!(network_message)),
                "expected network classification for: {network_message}"
            );
        }

        for fatal_message in [
            "openai http 401: invalid_api_key",
            "anthropic http 403: forbidden",
            "mistral http 429: rate limit exceeded",
            "provider openai not configured",
            "env var OPENAI_API_KEY not set",
            "api error: invalid x-api-key",
        ] {
            assert!(
                !is_network_validation_error(&anyhow::anyhow!(fatal_message)),
                "expected fatal classification for: {fatal_message}"
            );
        }
    }

    #[test]
    fn detects_missing_provider_key_error() {
        let err = anyhow::anyhow!("env var OPENAI_API_KEY not set");
        let missing = missing_provider_key_from_error("openai", &err).unwrap();
        assert_eq!(missing.provider, "openai");
        assert_eq!(missing.env_var, "OPENAI_API_KEY");
    }

    #[test]
    fn first_launch_decline_falls_back_to_manual_instructions() {
        assert_eq!(
            decide_first_launch_action(true, true, false),
            FirstLaunchAction::PrintManualInstructionsAndExit
        );
    }

    #[test]
    fn first_launch_noninteractive_falls_back_to_manual_instructions() {
        assert_eq!(
            decide_first_launch_action(true, false, false),
            FirstLaunchAction::PrintManualInstructionsAndExit
        );
    }

    #[test]
    fn missing_key_recovery_decline_returns_setup_hint() {
        let err = anyhow::anyhow!("env var OPENAI_API_KEY not set");
        assert_eq!(
            decide_missing_key_recovery_action("openai:gpt-4.1-mini", &err, true, false),
            MissingKeyRecoveryAction::ReturnErrorWithSetupHint
        );
    }

    #[test]
    fn missing_key_recovery_noninteractive_returns_setup_hint() {
        let err = anyhow::anyhow!("env var OPENAI_API_KEY not set");
        assert_eq!(
            decide_missing_key_recovery_action("openai:gpt-4.1-mini", &err, false, false),
            MissingKeyRecoveryAction::ReturnErrorWithSetupHint
        );
    }

    #[test]
    fn session_started_event_uses_final_effective_config() {
        let mut cfg = Config::defaults();
        cfg.default_model = "anthropic:claude-sonnet-4-20250514".into();
        cfg.policy.unknown = "deny".into();

        let event = session_started_event("session-123".into(), Path::new("/tmp/workspace"), &cfg);

        match event {
            AuditEvent::SessionStarted {
                session_id,
                cwd,
                model,
                policy_mode,
                config_effective_hash,
                ..
            } => {
                assert_eq!(session_id, "session-123");
                assert_eq!(cwd, "/tmp/workspace");
                assert_eq!(model, "anthropic:claude-sonnet-4-20250514");
                assert_eq!(policy_mode, "deny");
                assert_eq!(config_effective_hash, cfg.effective_hash());
            }
            other => panic!("expected SessionStarted, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn aborting_after_network_validation_failure_does_not_write_config_or_profile() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_path = tmp.path().join("config.toml");
        let profile_path = tmp.path().join(".zshrc");
        let old_home = std::env::var_os("HOME");
        let old_shell = std::env::var_os("SHELL");
        std::env::set_var("HOME", tmp.path());
        std::env::set_var("SHELL", "/bin/zsh");

        let mut prompts = FakePrompts {
            provider: Some("openai".into()),
            api_key: Some("sk-test".into()),
            model: Some("gpt-4.1-mini".into()),
            persist_env: true,
        };

        let result = run_interactive_setup_with(
            &cfg_path,
            &mut prompts,
            |_, _| {},
            |_path, _outcome| async {
                Err(SetupValidationError::Network(anyhow::anyhow!(
                    "network down"
                )))
            },
            || Ok(false),
        )
        .await;

        if let Some(home) = old_home {
            std::env::set_var("HOME", home);
        } else {
            std::env::remove_var("HOME");
        }
        if let Some(shell) = old_shell {
            std::env::set_var("SHELL", shell);
        } else {
            std::env::remove_var("SHELL");
        }

        assert!(result.is_err());
        assert!(!cfg_path.exists());
        assert!(!profile_path.exists());
    }

    #[tokio::test]
    async fn local_provider_construction_errors_do_not_offer_finish_anyway() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg_path = tmp.path().join("config.toml");
        let mut prompts = FakePrompts {
            provider: Some("openai".into()),
            api_key: Some("sk-test".into()),
            model: Some("gpt-4.1-mini".into()),
            persist_env: false,
        };
        let confirm_calls = Arc::new(AtomicUsize::new(0));
        let confirm_calls_for_closure = confirm_calls.clone();

        let result = run_interactive_setup_with(
            &cfg_path,
            &mut prompts,
            |_, _| {},
            |_path, _outcome| async {
                Err(SetupValidationError::Local(anyhow::anyhow!(
                    "env var OPENAI_API_KEY not set"
                )))
            },
            move || {
                confirm_calls_for_closure.fetch_add(1, Ordering::SeqCst);
                Ok(true)
            },
        )
        .await;

        assert!(result.is_err());
        assert_eq!(confirm_calls.load(Ordering::SeqCst), 0);
        assert!(!cfg_path.exists());
    }
}
