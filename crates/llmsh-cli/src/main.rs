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
use llmsh_core::setup::{run_setup_flow, SetupPrompts, SetupProvider};
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
    let (mut cfg, created) = load_or_create_user(&cfg_path)?;
    if created {
        println!("No llmsh config found.");
        println!("Created {}.", cfg_path.display());
        println!();
        println!("Set OPENAI_API_KEY to use the default OpenAI-compatible provider,");
        println!("ANTHROPIC_API_KEY for the Anthropic provider (Claude Haiku/Sonnet/Opus),");
        println!("or MISTRAL_API_KEY for the Mistral provider:");
        println!();
        println!("  export OPENAI_API_KEY=...");
        println!("  export ANTHROPIC_API_KEY=...");
        println!("  export MISTRAL_API_KEY=...");
        println!();
        println!("Then run: llmsh");
        return Ok(());
    }
    if let Some(m) = cli.model {
        cfg.default_model = m;
    }

    let workspace_root = std::env::current_dir()?;
    if let Some(project) = load_project(&workspace_root)? {
        let _ = merge_project(&mut cfg, &project);
    }

    let verbose_level: u8 = if cli.verbose > 0 {
        cli.verbose.min(2)
    } else if let Ok(s) = std::env::var("LLMSH_VERBOSE") {
        s.trim().parse::<u8>().unwrap_or(0).min(2)
    } else {
        cfg.verbose.default_level.min(2)
    };

    // 2. Audit
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
    writer.write(&AuditEvent::SessionStarted {
        ts: now_iso(),
        session_id: session_id.clone(),
        cwd: workspace_root.display().to_string(),
        model: cfg.default_model.clone(),
        policy_mode: cfg.policy.unknown.clone(),
        llmsh_version: env!("CARGO_PKG_VERSION").into(),
        schema_version: llmsh_audit::event::SCHEMA_VERSION,
        config_effective_hash: cfg.effective_hash(),
    })?;

    // 3. Provider
    // Chain: inner concrete provider → SwappableProvider (hot-swap shim) →
    // ThinkingProvider (status indicator). The SwappableProvider handle is
    // kept in the REPL so `/provider set …` can replace the inner provider.
    let (inner, shared_model, provider_prefix) = build_provider(&cfg)?;
    let swappable = Arc::new(llmsh_core::swappable::SwappableProvider::new(
        inner,
        shared_model.clone(),
    ));
    let provider: Arc<dyn LlmProvider> = Arc::new(llmsh_core::thinking::ThinkingProvider::new(
        swappable.clone() as Arc<dyn LlmProvider>,
    ));

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
        let value = read_trimmed_line("> ")?;
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
    let mut prompts = StdinSetupPrompts;
    let outcome = run_setup_flow(config_path, &mut prompts, |key, value| {
        std::env::set_var(key, value);
    })?;

    let (cfg, _) = load_or_create_user(config_path)?;
    match build_inner_provider(&outcome.provider, &outcome.model, &cfg) {
        Ok(provider) => {
            if let Err(err) = provider.list_models().await {
                eprintln!("Provider validation failed: {err}");
                if !confirm_finish_anyway()? {
                    anyhow::bail!("setup aborted after provider validation failure");
                }
            }
        }
        Err(err) => {
            eprintln!("Provider validation failed: {err}");
            if !confirm_finish_anyway()? {
                anyhow::bail!("setup aborted after provider validation failure");
            }
        }
    }

    println!(
        "Saved default model {}:{} in {}.",
        outcome.provider,
        outcome.model,
        config_path.display()
    );
    if let Some(profile) = outcome.profile_updated {
        println!("Updated {}.", profile.display());
    }
    Ok(())
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

fn confirm_finish_anyway() -> anyhow::Result<bool> {
    println!("Finish setup anyway? [y/N]");
    let answer = read_trimmed_line("> ")?;
    Ok(matches!(answer.to_ascii_lowercase().as_str(), "y" | "yes"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

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
}
