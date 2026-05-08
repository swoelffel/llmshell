use anyhow::Context as _;
use std::path::Path;

pub fn set_default_model(config_path: &Path, new_value: &str) -> anyhow::Result<()> {
    let original = std::fs::read_to_string(config_path)
        .with_context(|| format!("read config {}", config_path.display()))?;
    let mut doc = original
        .parse::<toml_edit::DocumentMut>()
        .with_context(|| format!("parse config {}", config_path.display()))?;
    doc["default_model"] = toml_edit::value(new_value);
    let serialized = doc.to_string();
    let tmp = config_path.with_extension("toml.tmp");
    std::fs::write(&tmp, &serialized).with_context(|| "write tmp config")?;
    std::fs::rename(&tmp, config_path).with_context(|| "rename tmp config")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(config_path, perms)
            .with_context(|| format!("set perms on {}", config_path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_preserves_comments_and_keys() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        let original = r#"# Top comment
default_model = "openai:gpt-4o-mini"
# Another comment
[providers.openai]
api_key_env = "OPENAI_API_KEY"
base_url = "https://api.openai.com/v1"
tool_calling = "native"
"#;
        std::fs::write(&path, original).unwrap();
        set_default_model(&path, "openai:gpt-4o").unwrap();
        let result = std::fs::read_to_string(&path).unwrap();
        assert!(result.contains("default_model = \"openai:gpt-4o\""));
        assert!(result.contains("# Top comment"));
        assert!(result.contains("# Another comment"));
        assert!(result.contains("api_key_env = \"OPENAI_API_KEY\""));
    }

    #[test]
    fn idempotent_same_value() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        let original = "default_model = \"openai:gpt-4o-mini\"\n";
        std::fs::write(&path, original).unwrap();
        set_default_model(&path, "openai:gpt-4o-mini").unwrap();
        let after_first = std::fs::read_to_string(&path).unwrap();
        set_default_model(&path, "openai:gpt-4o-mini").unwrap();
        let after_second = std::fs::read_to_string(&path).unwrap();
        assert_eq!(after_first, after_second);
        assert!(after_first.contains("openai:gpt-4o-mini"));
    }

    #[test]
    fn missing_file_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nonexistent.toml");
        let result = set_default_model(&path, "gpt-4o");
        assert!(result.is_err());
        let msg = format!("{:#}", result.unwrap_err());
        assert!(msg.contains("read config"));
    }

    #[test]
    fn malformed_toml_returns_error() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        std::fs::write(&path, "not valid {{ toml >>>").unwrap();
        let result = set_default_model(&path, "gpt-4o");
        assert!(result.is_err());
    }
}
