//! `/provider` slash command: list configured providers, switch the active
//! one (hot-swap via `SwappableProvider`), and chain into `/model` for the
//! newly active provider.

use std::io::{BufRead, Write};
use std::path::Path;
use std::sync::Arc;

use crate::config::Config;
use crate::model_cmd::{ModelCommandContext, ModelListCache};
use crate::swappable::SwappableProvider;
use anyhow::Context as _;
use llmsh_audit::event::AuditEvent;
use llmsh_audit::writer::AuditWriter;

/// Factory that knows how to build a concrete provider for a given name.
/// Implemented in `llmsh-cli` (where the concrete provider crates are linked).
/// Returning a `Box<dyn LlmProvider>` lets the REPL stay provider-agnostic.
pub trait ProviderSwapper: Send + Sync {
    /// Build a new inner provider for `name` using `cfg`. The model id used
    /// when initialising the provider is `model`. Implementations are expected
    /// to validate api_key_env presence (when configured) and bail otherwise.
    fn build(
        &self,
        name: &str,
        model: &str,
        cfg: &Config,
    ) -> anyhow::Result<Arc<dyn llmsh_llm::provider::LlmProvider>>;
}

pub struct ProviderCommandContext<'a> {
    pub cfg: &'a Config,
    pub current_provider: &'a Option<String>,
    pub swappable: &'a Arc<SwappableProvider>,
    pub model_cache: &'a ModelListCache,
    pub audit: &'a std::sync::Mutex<AuditWriter>,
    pub config_path: Option<&'a Path>,
    pub swapper: &'a dyn ProviderSwapper,
}

pub async fn handle_provider_command(
    ctx: &mut ProviderCommandContext<'_>,
    args: &[String],
) -> anyhow::Result<Option<String>> {
    match args {
        [] => interactive_select(ctx).await,
        [a] if a == "list" => {
            list_providers(ctx);
            Ok(None)
        }
        [a, name] if a == "set" => set_provider_flow(ctx, name).await,
        _ => {
            println!("usage: /provider | /provider list | /provider set <name>");
            Ok(None)
        }
    }
}

fn list_providers(ctx: &ProviderCommandContext<'_>) {
    let mut names: Vec<&String> = ctx.cfg.providers.keys().collect();
    names.sort();
    let current = ctx.current_provider.as_deref().unwrap_or("");
    println!(
        "Configured providers (current: {}):",
        if current.is_empty() { "-" } else { current }
    );
    for (i, name) in names.iter().enumerate() {
        if name.as_str() == current {
            println!("  [{}] {}      <- active", i + 1, name);
        } else {
            println!("  [{}] {}", i + 1, name);
        }
    }
}

async fn interactive_select(
    ctx: &mut ProviderCommandContext<'_>,
) -> anyhow::Result<Option<String>> {
    interactive_select_from(ctx, &mut std::io::BufReader::new(std::io::stdin())).await
}

pub async fn interactive_select_from<R: BufRead>(
    ctx: &mut ProviderCommandContext<'_>,
    reader: &mut R,
) -> anyhow::Result<Option<String>> {
    let mut names: Vec<String> = ctx.cfg.providers.keys().cloned().collect();
    names.sort();
    if names.is_empty() {
        println!("no providers configured");
        return Ok(None);
    }
    let current = ctx.current_provider.clone().unwrap_or_default();
    println!(
        "Configured providers (current: {}):",
        if current.is_empty() { "-" } else { &current }
    );
    for (i, name) in names.iter().enumerate() {
        if name == &current {
            println!("  [{}] {}      <- active", i + 1, name);
        } else {
            println!("  [{}] {}", i + 1, name);
        }
    }
    let n = names.len();
    loop {
        print!("Select a provider [1-{}, Enter to keep, q to cancel]: ", n);
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        reader.read_line(&mut line)?;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return Ok(None);
        }
        if trimmed.eq_ignore_ascii_case("q") {
            println!("cancelled");
            return Ok(None);
        }
        match trimmed.parse::<usize>() {
            Ok(idx) if idx >= 1 && idx <= n => {
                let name = names[idx - 1].clone();
                return set_provider_flow_with_reader(ctx, &name, reader).await;
            }
            _ => println!("invalid selection, try again"),
        }
    }
}

/// Convenience wrapper: chains the model-selection step against stdin.
pub async fn set_provider_flow(
    ctx: &mut ProviderCommandContext<'_>,
    name: &str,
) -> anyhow::Result<Option<String>> {
    let mut reader = std::io::BufReader::new(std::io::stdin());
    set_provider_flow_with_reader(ctx, name, &mut reader).await
}

/// Returns the new active provider name on success (so the REPL can update
/// its `model_provider_prefix`), or `None` if nothing changed. The `reader`
/// is consumed by the chained `/model` interactive selection.
pub async fn set_provider_flow_with_reader<R: BufRead>(
    ctx: &mut ProviderCommandContext<'_>,
    name: &str,
    reader: &mut R,
) -> anyhow::Result<Option<String>> {
    let pcfg = match ctx.cfg.providers.get(name) {
        Some(p) => p,
        None => {
            eprintln!("unknown provider: {}", name);
            return Ok(None);
        }
    };
    let from = ctx.current_provider.clone().unwrap_or_default();
    if from == name {
        println!("provider already active: {}", name);
        return Ok(None);
    }

    // Pick the initial model for the new provider: first allowlist entry, or
    // the provider's reported `current_model` (which would be its built-in
    // default when freshly instantiated below — so allowlist is preferred).
    let initial_model = pcfg
        .models
        .first()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("provider {} has no models configured", name))?;

    let new_inner = ctx
        .swapper
        .build(name, &initial_model, ctx.cfg)
        .with_context(|| format!("build provider {}", name))?;
    ctx.swappable.swap(new_inner, &initial_model);

    let _ = ctx
        .audit
        .lock()
        .unwrap()
        .write(&AuditEvent::ProviderChanged {
            ts: llmsh_audit::event::now_iso(),
            from: from.clone(),
            to: name.to_string(),
        });

    // Persist the new provider:model into config.toml so the next launch
    // starts on the same provider.
    if let Some(path) = ctx.config_path {
        let stored = format!("{}:{}", name, initial_model);
        crate::config::persist::set_default_model(path, &stored)
            .with_context(|| format!("persist provider+model to {}", path.display()))?;
        println!(
            "provider set to {} (model: {}, persisted to {})",
            name,
            initial_model,
            path.display()
        );
    } else {
        println!("provider set to {} (model: {})", name, initial_model);
    }

    // Invalidate the model list cache (it was populated for the previous
    // provider).
    ctx.model_cache.invalidate();

    // Chain into /model interactive selection so the user can pick a model
    // for the new provider in one go.
    let model_label = ctx.swappable.shared_model();
    let model_ctx = ModelCommandContext {
        provider: ctx.swappable.as_ref(),
        model_label: &model_label,
        cache: ctx.model_cache,
        config_path: ctx.config_path,
        audit: ctx.audit,
        model_provider_prefix: Some(name.to_string()),
        allowed_models: &pcfg.models,
    };
    if let Err(e) = crate::model_cmd::interactive_select_from(&model_ctx, reader).await {
        eprintln!("/provider: model selection error: {}", e);
    }

    Ok(Some(name.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use llmsh_llm::capabilities::{Capabilities, ToolCallingMode};
    use llmsh_llm::provider::LlmProvider;
    use llmsh_llm::types::{FinishReason, LlmRequest, LlmResponse, ModelInfo};
    use std::sync::RwLock;

    struct StubProvider {
        name: String,
        model: RwLock<String>,
    }
    #[async_trait]
    impl LlmProvider for StubProvider {
        fn capabilities(&self) -> Capabilities {
            Capabilities {
                tool_calling: ToolCallingMode::None,
                supports_streaming: false,
                supports_json_mode: false,
                supports_parallel_tool_calls: false,
                supports_tool_choice_required: false,
                max_context_tokens: None,
            }
        }
        async fn complete(&self, _req: LlmRequest) -> anyhow::Result<LlmResponse> {
            Ok(LlmResponse {
                message: Some(format!("{}:{}", self.name, self.current_model())),
                tool_calls: vec![],
                finish_reason: FinishReason::Stop,
                usage: None,
            })
        }
        async fn list_models(&self) -> anyhow::Result<Vec<ModelInfo>> {
            // Return models matching the active stub's allowlist so the
            // chained interactive_select picks something deterministic.
            Ok(vec![ModelInfo {
                id: self.current_model(),
                owned_by: Some(self.name.clone()),
                created: None,
            }])
        }
        async fn set_model(&self, id: &str) -> anyhow::Result<()> {
            *self.model.write().unwrap() = id.into();
            Ok(())
        }
        fn current_model(&self) -> String {
            self.model.read().unwrap().clone()
        }
    }

    struct StubSwapper;
    impl ProviderSwapper for StubSwapper {
        fn build(
            &self,
            name: &str,
            model: &str,
            _cfg: &Config,
        ) -> anyhow::Result<Arc<dyn LlmProvider>> {
            Ok(Arc::new(StubProvider {
                name: name.into(),
                model: RwLock::new(model.into()),
            }))
        }
    }

    fn test_cfg() -> Config {
        let mut cfg = Config::defaults();
        // ensure both providers exist with their default allowlists; replace
        // the "ollama" allowlist to a single deterministic entry.
        cfg.providers.get_mut("ollama").unwrap().models = vec!["llama3.1:8b".into()];
        cfg
    }

    #[tokio::test]
    async fn list_does_not_crash_with_no_current() {
        let cfg = test_cfg();
        let initial: Arc<dyn LlmProvider> = Arc::new(StubProvider {
            name: "openai".into(),
            model: RwLock::new("gpt-4.1-mini".into()),
        });
        let shared = Arc::new(RwLock::new("gpt-4.1-mini".into()));
        let sp = Arc::new(SwappableProvider::new(initial, shared));
        let cache = ModelListCache::new();
        let writer = std::sync::Mutex::new(AuditWriter::disabled());
        let swapper = StubSwapper;
        let current = Some("openai".into());
        let ctx = ProviderCommandContext {
            cfg: &cfg,
            current_provider: &current,
            swappable: &sp,
            model_cache: &cache,
            audit: &writer,
            config_path: None,
            swapper: &swapper,
        };
        list_providers(&ctx);
    }

    #[tokio::test]
    async fn set_unknown_provider_is_ignored() {
        let cfg = test_cfg();
        let initial: Arc<dyn LlmProvider> = Arc::new(StubProvider {
            name: "openai".into(),
            model: RwLock::new("gpt-4.1-mini".into()),
        });
        let shared = Arc::new(RwLock::new("gpt-4.1-mini".into()));
        let sp = Arc::new(SwappableProvider::new(initial, shared.clone()));
        let cache = ModelListCache::new();
        let writer = std::sync::Mutex::new(AuditWriter::disabled());
        let swapper = StubSwapper;
        let current = Some("openai".into());
        let mut ctx = ProviderCommandContext {
            cfg: &cfg,
            current_provider: &current,
            swappable: &sp,
            model_cache: &cache,
            audit: &writer,
            config_path: None,
            swapper: &swapper,
        };
        let result = set_provider_flow(&mut ctx, "does-not-exist").await.unwrap();
        assert_eq!(result, None);
        assert_eq!(*shared.read().unwrap(), "gpt-4.1-mini");
    }
}
