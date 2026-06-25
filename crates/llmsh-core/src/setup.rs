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
}
