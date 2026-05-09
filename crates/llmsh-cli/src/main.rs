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
use llmsh_llm::provider::LlmProvider;
use llmsh_llm_openai::provider::{OpenAIConfig, OpenAIProvider};
use llmsh_policy::context::PolicyContext;
use llmsh_policy::engine::{DefaultPolicyConfig, DefaultPolicyEngine, PolicyEngine, RiskAction};
use llmsh_policy::types::RiskLevel;
use llmsh_tools::list_directory::ListDirectory;
use llmsh_tools::read_file::ReadFile;
use llmsh_tools::registry::ToolRegistry;
use llmsh_tools::run_process::RunProcess;
use std::path::PathBuf;
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

    // 1. Load config
    let cfg_path = user_config_path(cli.config.as_deref())
        .ok_or_else(|| anyhow::anyhow!("could not determine config dir"))?;
    let (mut cfg, created) = load_or_create_user(&cfg_path)?;
    if created {
        println!("No llmsh config found.");
        println!("Created {}.", cfg_path.display());
        println!();
        println!("Set OPENAI_API_KEY to use the default OpenAI-compatible provider:");
        println!();
        println!("  export OPENAI_API_KEY=...");
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
    let (provider, shared_model, provider_prefix) = build_provider(&cfg)?;
    let provider: Arc<dyn LlmProvider> =
        Arc::new(llmsh_core::thinking::ThinkingProvider::new(provider));

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
    };
    repl.run().await?;
    Ok(())
}

fn build_provider(cfg: &Config) -> anyhow::Result<ProviderWithModel> {
    let (provider_name, model) = cfg
        .default_model
        .split_once(':')
        .ok_or_else(|| anyhow::anyhow!("default_model must be \"provider:model\""))?;
    let pcfg = cfg
        .providers
        .get(provider_name)
        .ok_or_else(|| anyhow::anyhow!("provider {} not configured", provider_name))?;
    let api_key = std::env::var(&pcfg.api_key_env)
        .map_err(|_| anyhow::anyhow!("env var {} not set", pcfg.api_key_env))?;
    let p = OpenAIProvider::new(OpenAIConfig {
        base_url: pcfg.base_url.clone(),
        api_key,
        model: model.into(),
        timeout_ms: 60_000,
    })?;
    let shared = p.shared_model();
    Ok((Arc::new(p), shared, Some(provider_name.to_string())))
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
