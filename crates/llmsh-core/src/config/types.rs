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
    #[serde(default)]
    pub verbose: VerboseConfig,
    #[serde(default)]
    pub compact: CompactConfig,
    #[serde(default)]
    pub memory: MemoryConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Name of the env var holding the API key for this provider.
    /// Optional: providers like local Ollama require no auth.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key_env: Option<String>,
    pub base_url: String,
    pub tool_calling: String,
    /// Curated allowlist of model ids surfaced via `/model`. When empty, the
    /// provider's `list_models()` response is used unfiltered (compat with
    /// pre-v0.2.11 configs that lack this field).
    #[serde(default)]
    pub models: Vec<String>,
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
    #[serde(default)]
    pub run_process: RunProcessPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RunProcessPolicy {
    /// When true (default), `run_process` invocations matching the
    /// `safe_commands` allowlist are downgraded from Unknown → ReadOnly.
    pub auto_classify_read_only: bool,
}

impl Default for RunProcessPolicy {
    fn default() -> Self {
        Self {
            auto_classify_read_only: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilesystemPolicy {
    pub allowed_roots: Vec<String>,
    /// Deprecated as of v0.2.7 — workspace boundary removed. Kept for
    /// backwards-compatible config parsing.
    #[serde(default)]
    pub allow_outside_workspace: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SensitivePathsPolicy {
    /// Deprecated as of v0.2.7 — sensitive paths now always require strong
    /// confirmation. Kept for backwards-compatible config parsing.
    #[serde(default = "default_sensitive_action")]
    pub action: String,
    pub patterns: Vec<String>,
}

fn default_sensitive_action() -> String {
    "confirm_strong".into()
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerboseConfig {
    /// Default verbose level when CLI flags / env are absent. 0 = silent.
    #[serde(default)]
    pub default_level: u8,
    /// Whether the reedline status line is rendered.
    #[serde(default = "default_status_line")]
    pub status_line: bool,
}

fn default_status_line() -> bool {
    true
}

impl Default for VerboseConfig {
    fn default() -> Self {
        Self {
            default_level: 0,
            status_line: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactConfig {
    /// Auto-compaction threshold as a percentage of the model's context window.
    /// 0 disables auto. Range 0-100; values > 100 are clamped to 100.
    #[serde(default = "default_auto_threshold")]
    pub auto_threshold_pct: u32,
    /// Number of trailing user turns to keep verbatim during summarization.
    #[serde(default = "default_keep_user_turns")]
    pub keep_last_user_turns: usize,
    /// Per-tool-result byte budget for the deterministic truncate stage.
    #[serde(default = "default_tool_output_max_bytes")]
    pub tool_output_max_bytes: usize,
    /// Cap on the summary length for the summarize stage.
    #[serde(default = "default_summary_max_tokens")]
    pub summary_max_tokens: u32,
    /// Provider:model id used for the summarization call. Empty → reuse the
    /// session model.
    #[serde(default)]
    pub model: String,
}

fn default_auto_threshold() -> u32 {
    80
}
fn default_keep_user_turns() -> usize {
    4
}
fn default_tool_output_max_bytes() -> usize {
    2048
}
fn default_summary_max_tokens() -> u32 {
    500
}

impl Default for CompactConfig {
    fn default() -> Self {
        Self {
            auto_threshold_pct: default_auto_threshold(),
            keep_last_user_turns: default_keep_user_turns(),
            tool_output_max_bytes: default_tool_output_max_bytes(),
            summary_max_tokens: default_summary_max_tokens(),
            model: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    /// Hard cap on the long-term facts list. The compactor LLM is asked to
    /// curate down to this number.
    #[serde(default = "default_max_facts")]
    pub max_facts: usize,
    /// When true, the active conversation is reloaded from SQLite at startup.
    /// Set to false to opt out (each launch starts fresh).
    #[serde(default = "default_auto_load")]
    pub auto_load_conversation: bool,
}

fn default_max_facts() -> usize {
    100
}
fn default_auto_load() -> bool {
    true
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            max_facts: default_max_facts(),
            auto_load_conversation: default_auto_load(),
        }
    }
}

impl Config {
    pub fn defaults() -> Self {
        let mut providers = HashMap::new();
        providers.insert(
            "openai".into(),
            ProviderConfig {
                api_key_env: Some("OPENAI_API_KEY".into()),
                base_url: "https://api.openai.com/v1".into(),
                tool_calling: "native".into(),
                models: vec![
                    "gpt-5".into(),
                    "gpt-5-mini".into(),
                    "gpt-4.1".into(),
                    "gpt-4.1-mini".into(),
                    "o3".into(),
                    "o4-mini".into(),
                ],
            },
        );
        providers.insert(
            "ollama".into(),
            ProviderConfig {
                api_key_env: None,
                base_url: "http://localhost:11434".into(),
                tool_calling: "native".into(),
                models: vec![
                    "llama3.1:8b".into(),
                    "qwen2.5-coder:7b".into(),
                    "qwen2.5-coder:32b".into(),
                    "mistral-nemo:12b".into(),
                    "qwen3:14b".into(),
                ],
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
                max_tool_calls_per_iteration: 32,
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
                        // SSH
                        "~/.ssh/**".into(),
                        "**/id_rsa".into(),
                        "**/id_ed25519".into(),
                        "**/id_ecdsa".into(),
                        // Cloud provider creds
                        "~/.aws/**".into(),
                        "~/.config/gcloud/**".into(),
                        "~/.config/gh/**".into(),
                        "~/.docker/config.json".into(),
                        "~/.kube/**".into(),
                        // Generic dotfiles
                        "~/.netrc".into(),
                        "~/.pgpass".into(),
                        // Project secrets
                        ".env".into(),
                        ".env.*".into(),
                        "**/.env".into(),
                        "**/.env.*".into(),
                        "**/credentials*".into(),
                        "**/secrets.*".into(),
                        "**/*.pem".into(),
                        "**/*.key".into(),
                        // System sensitive
                        "/etc/sudoers".into(),
                        "/etc/sudoers.d/**".into(),
                        "/etc/shadow".into(),
                        "/etc/passwd".into(),
                    ],
                },
                run_process: RunProcessPolicy::default(),
            },
            audit: AuditConfig {
                enabled: true,
                directory: "~/.llmsh/sessions".into(),
                redaction: RedactionConfig { enabled: true },
            },
            verbose: VerboseConfig::default(),
            compact: CompactConfig::default(),
            memory: MemoryConfig::default(),
        }
    }

    pub fn effective_hash(&self) -> String {
        let s = serde_json::to_string(&self).unwrap();
        llmsh_audit::digest::sha256_hex(s.as_bytes())
    }
}
