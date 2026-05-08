use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub default_model: String,
    pub providers: HashMap<String, ProviderConfig>,
    pub shell: ShellConfig,
    pub ui: UiConfig,
    pub limits: LimitsConfig,
    pub policy: PolicyConfig,
    pub audit: AuditConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub api_key_env: String,
    pub base_url: String,
    pub tool_calling: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShellConfig {
    pub raw_shell: Option<String>,
    pub raw_shell_args: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiConfig {
    pub show_plan: bool,
    pub show_tool_calls: bool,
    pub show_token_usage: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LimitsConfig {
    pub max_iterations: u32,
    pub max_tool_calls_per_iteration: u32,
    pub max_schema_repair_attempts: u32,
    pub max_llm_output_bytes: usize,
    pub max_audit_output_bytes: usize,
    pub tool_timeout_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyConfig {
    pub read_only: String,
    pub low_risk: String,
    pub write: String,
    pub destructive: String,
    pub network: String,
    pub privileged: String,
    pub unknown: String,
    pub filesystem: FilesystemPolicy,
    pub sensitive_paths: SensitivePathsPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilesystemPolicy {
    pub allowed_roots: Vec<String>,
    pub allow_outside_workspace: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SensitivePathsPolicy {
    pub action: String,
    pub patterns: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditConfig {
    pub enabled: bool,
    pub directory: String,
    pub redaction: RedactionConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RedactionConfig {
    pub enabled: bool,
}

impl Config {
    pub fn defaults() -> Self {
        let mut providers = HashMap::new();
        providers.insert(
            "openai".into(),
            ProviderConfig {
                api_key_env: "OPENAI_API_KEY".into(),
                base_url: "https://api.openai.com/v1".into(),
                tool_calling: "native".into(),
            },
        );
        Self {
            default_model: "openai:gpt-4.1-mini".into(),
            providers,
            shell: ShellConfig {
                raw_shell: None,
                raw_shell_args: vec!["-lc".into()],
            },
            ui: UiConfig {
                show_plan: true,
                show_tool_calls: true,
                show_token_usage: false,
            },
            limits: LimitsConfig {
                max_iterations: 5,
                max_tool_calls_per_iteration: 5,
                max_schema_repair_attempts: 2,
                max_llm_output_bytes: 4096,
                max_audit_output_bytes: 65536,
                tool_timeout_ms: 30000,
            },
            policy: PolicyConfig {
                read_only: "allow".into(),
                low_risk: "allow".into(),
                write: "confirm".into(),
                destructive: "confirm_strong".into(),
                network: "confirm".into(),
                privileged: "deny".into(),
                unknown: "confirm".into(),
                filesystem: FilesystemPolicy {
                    allowed_roots: vec![".".into()],
                    allow_outside_workspace: false,
                },
                sensitive_paths: SensitivePathsPolicy {
                    action: "deny".into(),
                    patterns: vec![
                        "~/.ssh/**".into(),
                        "~/.aws/**".into(),
                        "~/.config/gcloud/**".into(),
                        ".env*".into(),
                        "**/id_rsa".into(),
                        "**/id_ed25519".into(),
                        "**/credentials*".into(),
                        "**/secrets.*".into(),
                    ],
                },
            },
            audit: AuditConfig {
                enabled: true,
                directory: "~/.llmsh/sessions".into(),
                redaction: RedactionConfig { enabled: true },
            },
        }
    }

    pub fn effective_hash(&self) -> String {
        let s = serde_json::to_string(&self).unwrap();
        llmsh_audit::digest::sha256_hex(s.as_bytes())
    }
}
