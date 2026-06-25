use crate::config::Config;
use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};

const MANAGED_BLOCK_START: &str = "# >>> llmsh setup >>>";
const MANAGED_BLOCK_END: &str = "# <<< llmsh setup <<<";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShellProfileSyntax {
    Posix,
    Fish,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupProvider {
    pub name: String,
    pub display_name: String,
    pub api_key_env: Option<String>,
}

pub trait SetupPrompts {
    fn choose_provider(&mut self, providers: &[SetupProvider]) -> Result<Option<String>>;
    fn read_api_key(&mut self, provider: &SetupProvider) -> Result<Option<String>>;
    fn choose_model(&mut self, provider: &str, models: &[String]) -> Result<Option<String>>;
    fn confirm_persist_env(&mut self, profile: &Path, env_var: &str) -> Result<bool>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupOutcome {
    pub provider: String,
    pub model: String,
    pub env_var: Option<String>,
    pub env_value: Option<String>,
    pub persist_env: bool,
    pub profile_path: Option<PathBuf>,
}

pub fn run_setup_flow(
    config_path: &Path,
    prompts: &mut impl SetupPrompts,
    env_setter: impl FnMut(&str, &str),
) -> Result<SetupOutcome> {
    let profile = detected_profile_from_env();
    run_setup_flow_with_profile(config_path, prompts, profile, env_setter)
}

fn detected_profile_from_env() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    detect_shell_profile(&home, std::env::var("SHELL").ok().as_deref())
}

fn run_setup_flow_with_profile(
    config_path: &Path,
    prompts: &mut impl SetupPrompts,
    profile: Option<PathBuf>,
    mut env_setter: impl FnMut(&str, &str),
) -> Result<SetupOutcome> {
    let cfg = load_existing_or_default_config(config_path)?;
    let providers = available_providers(&cfg);
    if providers.is_empty() {
        return Err(anyhow!("no providers configured"));
    }
    let provider_name = prompts
        .choose_provider(&providers)?
        .ok_or_else(|| anyhow!("setup canceled before provider selection"))?;
    let provider = providers
        .iter()
        .find(|candidate| candidate.name == provider_name)
        .ok_or_else(|| anyhow!("unknown provider: {provider_name}"))?;

    let mut env_value = None;
    let mut persist_env = false;
    if let Some(env_var) = provider.api_key_env.as_deref() {
        if std::env::var(env_var).is_err() {
            let api_key = prompts
                .read_api_key(provider)?
                .ok_or_else(|| anyhow!("setup canceled before API key entry"))?;
            env_setter(env_var, &api_key);
            env_value = Some(api_key);
            if let Some(profile_path) = profile.as_ref() {
                persist_env = prompts.confirm_persist_env(profile_path, env_var)?;
            }
        }
    }

    let models = cfg
        .providers
        .get(&provider_name)
        .ok_or_else(|| anyhow!("provider {provider_name} not configured"))?
        .models
        .clone();
    if models.is_empty() {
        return Err(anyhow!("{provider_name} has no configured models"));
    }
    let model = match prompts.choose_model(&provider_name, &models)? {
        Some(selected) => {
            if !models.iter().any(|candidate| candidate == &selected) {
                return Err(anyhow!(
                    "unknown model {selected} for provider {provider_name}"
                ));
            }
            selected
        }
        None => models[0].clone(),
    };

    Ok(SetupOutcome {
        provider: provider_name,
        model,
        env_var: provider.api_key_env.clone(),
        env_value,
        persist_env,
        profile_path: profile,
    })
}

pub fn load_existing_or_default_config(config_path: &Path) -> Result<Config> {
    if config_path.exists() {
        let s = std::fs::read_to_string(config_path)?;
        Ok(toml::from_str(&s).unwrap_or_else(|_| Config::defaults()))
    } else {
        Ok(Config::defaults())
    }
}

pub fn finalize_setup(config_path: &Path, outcome: &SetupOutcome) -> Result<Option<PathBuf>> {
    let _ = crate::config::load::load_or_create_user(config_path)?;
    crate::config::persist::set_default_model_and_provider(
        config_path,
        &crate::config::Config::defaults(),
        &outcome.provider,
        &outcome.model,
    )?;

    if outcome.persist_env {
        if let (Some(profile_path), Some(env_var), Some(env_value)) = (
            outcome.profile_path.as_ref(),
            outcome.env_var.as_deref(),
            outcome.env_value.as_deref(),
        ) {
            upsert_managed_env_block(profile_path, env_var, env_value)?;
            return Ok(Some(profile_path.clone()));
        }
    }

    Ok(None)
}

pub fn available_providers(cfg: &Config) -> Vec<SetupProvider> {
    let mut providers: Vec<_> = cfg
        .providers
        .iter()
        .map(|(name, provider)| SetupProvider {
            name: name.clone(),
            display_name: name.clone(),
            api_key_env: provider.api_key_env.clone(),
        })
        .collect();
    providers.sort_by(|left, right| left.name.cmp(&right.name));
    providers
}

pub fn default_model_for_provider(cfg: &Config, provider: &str) -> Result<String> {
    let provider_cfg = cfg
        .providers
        .get(provider)
        .ok_or_else(|| anyhow!("unknown provider: {provider}"))?;
    provider_cfg
        .models
        .first()
        .cloned()
        .ok_or_else(|| anyhow!("{provider} has no configured models"))
}

pub fn detect_shell_profile(home: &Path, shell: Option<&str>) -> Option<PathBuf> {
    let profile = shell
        .and_then(|shell_path| Path::new(shell_path).file_name())
        .and_then(|name| name.to_str())
        .map(|name| match name {
            shell if shell.ends_with("zsh") => home.join(".zshrc"),
            shell if shell.ends_with("bash") => home.join(".bashrc"),
            shell if shell.ends_with("fish") => {
                let fish_profile = home.join(".config/fish/config.fish");
                if fish_profile.exists() {
                    fish_profile
                } else {
                    home.join(".profile")
                }
            }
            _ => home.join(".profile"),
        })
        .unwrap_or_else(|| home.join(".profile"));
    Some(profile)
}

fn shell_escape_single_quoted(value: &str) -> String {
    value.replace('\'', "'\"'\"'")
}

fn fish_escape_single_quoted(value: &str) -> String {
    value.replace('\\', "\\\\").replace('\'', "\\'")
}

fn detect_profile_syntax(profile: &Path) -> ShellProfileSyntax {
    if profile.file_name().and_then(|name| name.to_str()) == Some("config.fish")
        && profile
            .parent()
            .and_then(|parent| parent.file_name())
            .and_then(|name| name.to_str())
            == Some("fish")
    {
        ShellProfileSyntax::Fish
    } else {
        ShellProfileSyntax::Posix
    }
}

pub fn render_managed_env_block(env_var: &str, value: &str) -> String {
    render_managed_env_block_for_syntax(ShellProfileSyntax::Posix, env_var, value)
}

fn render_managed_env_block_for_syntax(
    syntax: ShellProfileSyntax,
    env_var: &str,
    value: &str,
) -> String {
    let export_line = match syntax {
        ShellProfileSyntax::Posix => {
            format!("export {env_var}='{}'", shell_escape_single_quoted(value))
        }
        ShellProfileSyntax::Fish => {
            format!("set -gx {env_var} '{}'", fish_escape_single_quoted(value))
        }
    };

    format!("{MANAGED_BLOCK_START}\n{export_line}\n{MANAGED_BLOCK_END}\n")
}

pub fn upsert_managed_env_block(profile: &Path, env_var: &str, value: &str) -> Result<()> {
    let existing = if profile.exists() {
        std::fs::read_to_string(profile)?
    } else {
        String::new()
    };

    let rendered =
        render_managed_env_block_for_syntax(detect_profile_syntax(profile), env_var, value);
    let updated = match (
        existing.find(MANAGED_BLOCK_START),
        existing.find(MANAGED_BLOCK_END),
    ) {
        (Some(start), Some(end)) if start <= end => {
            let end = end + MANAGED_BLOCK_END.len();
            let before = &existing[..start];
            let after = existing[end..]
                .strip_prefix('\n')
                .unwrap_or(&existing[end..]);
            format!("{before}{rendered}{after}")
        }
        _ => {
            if existing.is_empty() {
                rendered
            } else if existing.ends_with('\n') {
                format!("{existing}{rendered}")
            } else {
                format!("{existing}\n{rendered}")
            }
        }
    };

    std::fs::write(profile, updated)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakePrompts {
        provider: Option<String>,
        api_key: Option<String>,
        model: Option<String>,
        persist_env: bool,
        profile: std::path::PathBuf,
    }

    impl FakePrompts {
        fn new(
            provider: &str,
            api_key: &str,
            model: &str,
            persist_env: bool,
            profile: std::path::PathBuf,
        ) -> Self {
            Self {
                provider: Some(provider.to_string()),
                api_key: Some(api_key.to_string()),
                model: Some(model.to_string()),
                persist_env,
                profile,
            }
        }
    }

    impl SetupPrompts for FakePrompts {
        fn choose_provider(&mut self, _providers: &[SetupProvider]) -> Result<Option<String>> {
            Ok(self.provider.take())
        }

        fn read_api_key(&mut self, _provider: &SetupProvider) -> Result<Option<String>> {
            Ok(self.api_key.take())
        }

        fn choose_model(&mut self, _provider: &str, _models: &[String]) -> Result<Option<String>> {
            Ok(self.model.take())
        }

        fn confirm_persist_env(&mut self, profile: &Path, env_var: &str) -> Result<bool> {
            assert_eq!(profile, self.profile.as_path());
            assert_eq!(env_var, "OPENAI_API_KEY");
            Ok(self.persist_env)
        }
    }

    #[test]
    fn available_providers_are_stable_and_sorted() {
        let cfg = crate::config::Config::defaults();
        let names: Vec<_> = available_providers(&cfg)
            .into_iter()
            .map(|p| p.name)
            .collect();
        assert_eq!(names, vec!["anthropic", "mistral", "ollama", "openai"]);
    }

    #[test]
    fn default_model_uses_first_configured_model() {
        let cfg = crate::config::Config::defaults();
        assert_eq!(
            default_model_for_provider(&cfg, "openai").unwrap(),
            cfg.providers["openai"].models[0]
        );
    }

    #[test]
    fn default_model_errors_for_empty_model_list() {
        let mut cfg = crate::config::Config::defaults();
        cfg.providers.get_mut("openai").unwrap().models.clear();
        let err = default_model_for_provider(&cfg, "openai")
            .unwrap_err()
            .to_string();
        assert!(err.contains("openai has no configured models"));
    }

    #[test]
    fn render_env_block_single_quotes_safely() {
        let block = render_managed_env_block("OPENAI_API_KEY", "sk-test'quote");
        assert!(block.contains("# >>> llmsh setup >>>"));
        assert!(block.contains("export OPENAI_API_KEY='sk-test'\"'\"'quote'"));
        assert!(block.contains("# <<< llmsh setup <<<"));
    }

    #[test]
    fn detect_shell_profile_prefers_existing_fish_config() {
        let tmp = tempfile::tempdir().unwrap();
        let fish_dir = tmp.path().join(".config/fish");
        std::fs::create_dir_all(&fish_dir).unwrap();
        let fish_profile = fish_dir.join("config.fish");
        std::fs::write(&fish_profile, "").unwrap();

        let detected = detect_shell_profile(tmp.path(), Some("/usr/local/bin/fish"));

        assert_eq!(detected, Some(fish_profile));
    }

    #[test]
    fn detect_shell_profile_fish_falls_back_to_profile_when_missing() {
        let tmp = tempfile::tempdir().unwrap();

        let detected = detect_shell_profile(tmp.path(), Some("/opt/homebrew/bin/fish"));

        assert_eq!(detected, Some(tmp.path().join(".profile")));
    }

    #[test]
    fn detect_shell_profile_defaults_to_profile_when_shell_missing() {
        let tmp = tempfile::tempdir().unwrap();

        let detected = detect_shell_profile(tmp.path(), None);

        assert_eq!(detected, Some(tmp.path().join(".profile")));
    }

    #[test]
    fn upsert_env_block_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let profile = tmp.path().join(".zshrc");
        std::fs::write(&profile, "export PATH=\"$HOME/bin:$PATH\"\n").unwrap();
        upsert_managed_env_block(&profile, "OPENAI_API_KEY", "sk-one").unwrap();
        upsert_managed_env_block(&profile, "OPENAI_API_KEY", "sk-two").unwrap();
        let result = std::fs::read_to_string(&profile).unwrap();
        assert!(result.contains("export PATH="));
        assert_eq!(result.matches("# >>> llmsh setup >>>").count(), 1);
        assert!(result.contains("export OPENAI_API_KEY='sk-two'"));
        assert!(!result.contains("sk-one"));
    }

    #[test]
    fn upsert_env_block_uses_fish_syntax_for_fish_profile() {
        let tmp = tempfile::tempdir().unwrap();
        let fish_dir = tmp.path().join(".config/fish");
        std::fs::create_dir_all(&fish_dir).unwrap();
        let profile = fish_dir.join("config.fish");
        std::fs::write(&profile, "set -gx PATH $HOME/bin $PATH\n").unwrap();

        upsert_managed_env_block(&profile, "OPENAI_API_KEY", "sk-test'value").unwrap();

        let result = std::fs::read_to_string(&profile).unwrap();
        assert!(result.contains("set -gx PATH $HOME/bin $PATH"));
        assert!(result.contains("set -gx OPENAI_API_KEY 'sk-test\\'value'"));
        assert!(!result.contains("export OPENAI_API_KEY"));
    }

    #[test]
    fn setup_flow_defers_persistence_until_finalize() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = tmp.path().join("config.toml");
        let profile = tmp.path().join(".zshrc");
        let mut prompts =
            FakePrompts::new("openai", "sk-test", "gpt-4.1-mini", true, profile.clone());
        let mut envs = Vec::new();

        let outcome =
            run_setup_flow_with_profile(&cfg, &mut prompts, Some(profile.clone()), |k, v| {
                envs.push((k.to_string(), v.to_string()));
            })
            .unwrap();

        assert_eq!(outcome.provider, "openai");
        assert_eq!(outcome.model, "gpt-4.1-mini");
        assert_eq!(outcome.env_var.as_deref(), Some("OPENAI_API_KEY"));
        assert_eq!(outcome.env_value.as_deref(), Some("sk-test"));
        assert!(outcome.persist_env);
        assert_eq!(outcome.profile_path.as_deref(), Some(profile.as_path()));
        assert!(!cfg.exists());
        assert!(!profile.exists());
        assert_eq!(envs, vec![("OPENAI_API_KEY".into(), "sk-test".into())]);

        let profile_updated = finalize_setup(&cfg, &outcome).unwrap();

        assert_eq!(profile_updated.as_deref(), Some(profile.as_path()));
        assert!(std::fs::read_to_string(&cfg)
            .unwrap()
            .contains("default_model = \"openai:gpt-4.1-mini\""));
        assert!(std::fs::read_to_string(&profile)
            .unwrap()
            .contains("export OPENAI_API_KEY='sk-test'"));
    }
}
