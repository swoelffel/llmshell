use anyhow::Context as _;
use std::path::Path;

pub fn set_default_model(config_path: &Path, new_value: &str) -> anyhow::Result<()> {
    let original = std::fs::read_to_string(config_path)
        .with_context(|| format!("read config {}", config_path.display()))?;
    let mut doc = original
        .parse::<toml_edit::DocumentMut>()
        .with_context(|| format!("parse config {}", config_path.display()))?;
    doc["default_model"] = toml_edit::value(new_value);
    write_config(config_path, &doc)?;
    Ok(())
}

pub fn set_default_model_and_provider(
    config_path: &Path,
    cfg_defaults: &crate::config::Config,
    provider: &str,
    model: &str,
) -> anyhow::Result<()> {
    let original = std::fs::read_to_string(config_path)
        .with_context(|| format!("read config {}", config_path.display()))?;
    let mut doc = original
        .parse::<toml_edit::DocumentMut>()
        .with_context(|| format!("parse config {}", config_path.display()))?;
    doc["default_model"] = toml_edit::value(format!("{provider}:{model}"));

    let providers = doc
        .as_table_mut()
        .entry("providers")
        .or_insert(toml_edit::table())
        .as_table_mut()
        .with_context(|| "providers must be a table")?;

    if !providers.contains_key(provider) {
        let provider_defaults = cfg_defaults
            .providers
            .get(provider)
            .with_context(|| format!("missing default provider config for {provider}"))?;
        let provider_doc = toml_edit::ser::to_document(provider_defaults)
            .with_context(|| format!("serialize default provider config for {provider}"))?;
        providers.insert(provider, provider_doc.into_item());
    }

    write_config(config_path, &doc)?;
    Ok(())
}

fn write_config(config_path: &Path, doc: &toml_edit::DocumentMut) -> anyhow::Result<()> {
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

    #[test]
    fn setup_patch_sets_default_model_and_preserves_existing_provider() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        std::fs::write(
            &path,
            r#"# keep me
default_model = "openai:gpt-4.1-mini"

[providers.openai]
api_key_env = "CUSTOM_OPENAI_KEY"
base_url = "https://proxy.example/v1"
tool_calling = "native"
models = ["custom-model"]
"#,
        )
        .unwrap();

        let defaults = crate::config::Config::defaults();
        set_default_model_and_provider(&path, &defaults, "openai", "custom-model").unwrap();
        let result = std::fs::read_to_string(&path).unwrap();
        assert!(result.contains("# keep me"));
        assert!(result.contains("default_model = \"openai:custom-model\""));
        assert!(result.contains("api_key_env = \"CUSTOM_OPENAI_KEY\""));
        assert!(result.contains("https://proxy.example/v1"));
    }

    #[test]
    fn setup_patch_adds_missing_provider_from_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        std::fs::write(&path, "default_model = \"openai:gpt-4.1-mini\"\n").unwrap();
        let defaults = crate::config::Config::defaults();
        set_default_model_and_provider(&path, &defaults, "mistral", "mistral-medium-3-5").unwrap();
        let result = std::fs::read_to_string(&path).unwrap();
        assert!(result.contains("default_model = \"mistral:mistral-medium-3-5\""));
        assert!(result.contains("[providers.mistral]"));
        assert!(result.contains("api_key_env = \"MISTRAL_API_KEY\""));
    }
}
