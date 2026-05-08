use super::types::Config;
use std::path::{Path, PathBuf};

pub struct LoadOutcome {
    pub config: Config,
    pub created_user_config: Option<PathBuf>,
    pub project_warnings: Vec<String>,
}

pub fn user_config_path(override_path: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = override_path {
        return Some(p.to_path_buf());
    }
    directories::ProjectDirs::from("", "", "llmsh").map(|d| d.config_dir().join("config.toml"))
}

pub fn load_or_create_user(path: &Path) -> anyhow::Result<(Config, bool)> {
    if path.exists() {
        let s = std::fs::read_to_string(path)?;
        let cfg: Config = toml::from_str(&s).unwrap_or_else(|_| Config::defaults());
        return Ok((cfg, false));
    }
    let cfg = Config::defaults();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let toml_str = toml::to_string_pretty(&cfg)?;
    std::fs::write(path, toml_str)?;
    set_perms(path, 0o600)?;
    Ok((cfg, true))
}

#[cfg(unix)]
fn set_perms(p: &Path, mode: u32) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode))?;
    Ok(())
}

#[cfg(not(unix))]
fn set_perms(_: &Path, _: u32) -> anyhow::Result<()> {
    Ok(())
}

pub fn load_project(workspace_root: &Path) -> anyhow::Result<Option<toml::Value>> {
    let p = workspace_root.join(".llmsh.toml");
    if !p.exists() {
        return Ok(None);
    }
    let s = std::fs::read_to_string(&p)?;
    Ok(Some(toml::from_str(&s)?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_run_creates_with_0600() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("cfg.toml");
        let (_, created) = load_or_create_user(&p).unwrap();
        assert!(created);
        assert!(p.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let m = std::fs::metadata(&p).unwrap();
            assert_eq!(m.permissions().mode() & 0o777, 0o600);
        }
        let s = std::fs::read_to_string(&p).unwrap();
        assert!(!s.contains("OPENAI_API_KEY=")); // reference only, no secret
    }
}
