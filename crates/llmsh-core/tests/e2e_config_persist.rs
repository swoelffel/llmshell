use llmsh_core::config::persist::set_default_model;

#[test]
fn set_default_model_updates_value() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.toml");
    let original = r#"# My config
default_model = "openai:gpt-4o-mini"

[providers.openai]
api_key_env = "OPENAI_API_KEY"
base_url = "https://api.openai.com/v1"
tool_calling = "native"
"#;
    std::fs::write(&path, original).unwrap();
    set_default_model(&path, "openai:gpt-4o").unwrap();

    let result = std::fs::read_to_string(&path).unwrap();
    assert!(result.contains("default_model = \"openai:gpt-4o\""));
    assert!(!result.contains("gpt-4o-mini"));
}

#[test]
fn comments_and_extra_keys_preserved() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.toml");
    let original = r#"# This is a comment
# Second comment
default_model = "openai:gpt-4o-mini"
extra_key = "should-remain"

[providers.openai]
api_key_env = "OPENAI_API_KEY"
base_url = "https://api.openai.com/v1"
tool_calling = "native"
"#;
    std::fs::write(&path, original).unwrap();
    set_default_model(&path, "openai:gpt-4o").unwrap();

    let result = std::fs::read_to_string(&path).unwrap();
    assert!(
        result.contains("# This is a comment"),
        "first comment preserved"
    );
    assert!(
        result.contains("# Second comment"),
        "second comment preserved"
    );
    assert!(
        result.contains("extra_key = \"should-remain\""),
        "extra key preserved"
    );
    assert!(result.contains("api_key_env"), "provider section preserved");
}

#[test]
fn file_is_intact_after_write() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("config.toml");
    let original = "default_model = \"openai:gpt-4o-mini\"\n";
    std::fs::write(&path, original).unwrap();

    set_default_model(&path, "openai:gpt-4o").unwrap();

    // File exists and is valid TOML
    let result = std::fs::read_to_string(&path).unwrap();
    let parsed: toml::Value = toml::from_str(&result).expect("must be valid TOML");
    assert_eq!(parsed["default_model"].as_str(), Some("openai:gpt-4o"));
}
