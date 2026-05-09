mod common;

use llmsh_core::agent::AgentLoop;
use llmsh_core::context::ContextBuilder;
use llmsh_llm::types::{FinishReason, LlmResponse, ToolCall};
use llmsh_tools::read_file::ReadFile;
use llmsh_tools::registry::ToolRegistry;
use serde_json::json;
use std::sync::{Arc, RwLock};

const FAKE_OPENAI_KEY: &str = "sk-AAAAAAAAAAAAAAAAAAAAAAAA";
const FAKE_AWS_KEY: &str = "AKIAIOSFODNN7EXAMPLE";
// A minimal but valid-enough JWT structure (three base64url parts)
const FAKE_JWT: &str =
    "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJ1c2VyMTIzIn0.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";
const FAKE_PEM_BODY: &str = "MIIBfakePrivateKeyDataHere";

fn fake_secrets_content() -> String {
    format!(
        "openai_key={key}\naws={aws}\ntoken={jwt}\n-----BEGIN RSA PRIVATE KEY-----\n{pem}\n-----END RSA PRIVATE KEY-----\n",
        key = FAKE_OPENAI_KEY,
        aws = FAKE_AWS_KEY,
        jwt = FAKE_JWT,
        pem = FAKE_PEM_BODY,
    )
}

/// After reading a file containing multiple secret patterns, the audit JSONL
/// must contain `[REDACTED:*]` markers and must NOT contain any literal secret.
#[tokio::test]
async fn redaction_no_literal_secrets_in_audit() {
    let tmp = tempfile::tempdir().unwrap();
    // Canonicalize to avoid macOS /var -> /private/var symlink mismatches that
    // would cause the policy to treat the file as "outside workspace".
    let tmp_canonical =
        std::fs::canonicalize(tmp.path()).unwrap_or_else(|_| tmp.path().to_path_buf());
    let secret_file = tmp_canonical.join("secrets.txt");
    std::fs::write(&secret_file, fake_secrets_content()).unwrap();

    let scripted = vec![
        LlmResponse {
            message: None,
            tool_calls: vec![ToolCall {
                id: "c1".into(),
                name: "read_file".into(),
                args: json!({"path": secret_file.to_str().unwrap()}),
            }],
            finish_reason: FinishReason::ToolCalls,
            usage: None,
        },
        LlmResponse {
            message: Some("Done reading secrets file".into()),
            tool_calls: vec![],
            finish_reason: FinishReason::Stop,
            usage: None,
        },
    ];

    let mut reg = ToolRegistry::new();
    reg.register(Arc::new(ReadFile));
    let registry = Arc::new(reg);

    let audit_dir = tempfile::tempdir().unwrap();
    // The file is absolute so it will be resolved outside the workspace root
    // (tmp dir). We need to allow outside workspace or use the file path as cwd.
    // Use the file's parent as workspace root so it's "inside".
    let deps = {
        use llmsh_audit::redact::Redactor;
        use llmsh_audit::writer::AuditWriter;
        use llmsh_core::agent::{AgentBounds, AgentDeps};
        use llmsh_core::config::{CompactConfig, MemoryConfig};
        use llmsh_core::confirm::AlwaysYesGate;
        use llmsh_core::context::StaticSystemPrompt;
        use llmsh_core::executor::ToolExecutor;
        use llmsh_core::memory::Memory;
        use llmsh_core::pipeline::Pipeline;
        use llmsh_policy::context::PolicyContext;
        use llmsh_policy::engine::{DefaultPolicyConfig, DefaultPolicyEngine};
        use std::sync::Mutex;
        use tokio_util::sync::CancellationToken;

        let writer = AuditWriter::open(audit_dir.path(), "test-session").unwrap();
        let policy = Arc::new(DefaultPolicyEngine::new(DefaultPolicyConfig::default()));
        let pipeline = Pipeline {
            registry: registry.clone(),
            policy,
            home: None,
        };
        Arc::new(AgentDeps {
            provider: Arc::new(common::MockLlmProvider::new(scripted)),
            pipeline,
            executor: ToolExecutor {
                registry,
                timeout: std::time::Duration::from_secs(5),
                max_output_bytes: 65536,
                env: Default::default(),
                cancel: CancellationToken::new(),
                home: None,
            },
            gate: Arc::new(AlwaysYesGate),
            audit: Mutex::new(writer),
            redactor: Redactor::default_audit(),
            bounds: AgentBounds {
                max_iterations: 5,
                max_tool_calls_per_iteration: 5,
                max_schema_repair_attempts: 2,
            },
            compact_config: CompactConfig::default(),
            memory_cfg: MemoryConfig::default(),
            policy_ctx: PolicyContext {
                cwd: std::sync::Arc::new(std::sync::RwLock::new(tmp_canonical.clone())),
                workspace_root: tmp_canonical.clone(),
                allowed_roots: vec![tmp_canonical.clone()],
                sensitive_path_patterns: vec![],
            },
            sensitive_patterns: vec![],
            model_label: Arc::new(RwLock::new("mock:test".into())),
            system_prompt: Arc::new(StaticSystemPrompt::new(None)),
            memory: Arc::new(Memory::open_in_memory().unwrap()),
            verbose: 0,
            stats: Arc::new(std::sync::RwLock::new(
                llmsh_core::session_stats::SessionStats::default(),
            )),
            oldpwd: std::sync::Arc::new(std::sync::Mutex::new(None)),
            home: None,
        })
    };

    let mut agent = AgentLoop {
        deps: deps.clone(),
        builder: ContextBuilder::new(65536),
    };
    let res = agent.run("read the secrets file").await.unwrap();
    assert_eq!(res.stopped_reason, "stop");

    deps.audit.lock().unwrap().flush().unwrap();
    let log = std::fs::read_to_string(audit_dir.path().join("test-session.jsonl")).unwrap();

    // ---- Positive assertions: redaction markers must be present ----
    assert!(
        log.contains("[REDACTED:openai_key]"),
        "expected [REDACTED:openai_key] in audit log"
    );
    assert!(
        log.contains("[REDACTED:jwt]"),
        "expected [REDACTED:jwt] in audit log"
    );
    assert!(
        log.contains("[REDACTED:aws_access_key]"),
        "expected [REDACTED:aws_access_key] in audit log"
    );
    assert!(
        log.contains("[REDACTED:pem_private_key]"),
        "expected [REDACTED:pem_private_key] in audit log"
    );

    // ---- Negative assertions: literal secrets must NOT appear ----
    assert!(
        !log.contains("sk-AAAA"),
        "literal OpenAI key must not appear in audit log"
    );
    assert!(
        !log.contains("AKIA"),
        "literal AWS access key must not appear in audit log"
    );
    assert!(
        !log.contains(FAKE_PEM_BODY),
        "literal PEM body must not appear in audit log"
    );
    assert!(
        !log.contains(FAKE_JWT),
        "literal JWT must not appear in audit log"
    );
}
