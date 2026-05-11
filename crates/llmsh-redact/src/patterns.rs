//! Catalogue centralisé des patterns de secrets reconnus.
//!
//! Ajouter un secret = ajouter une entrée dans `default_patterns()`.
//! Le label `name` apparaît dans le marqueur `[REDACTED:<name>]`.

pub struct PatternDef {
    pub name: &'static str,
    pub regex: &'static str,
}

pub fn default_patterns() -> Vec<PatternDef> {
    vec![
        PatternDef {
            name: "anthropic_key",
            regex: r"sk-ant-[A-Za-z0-9_-]{20,}",
        },
        PatternDef {
            name: "openai_key",
            regex: r"sk-(?:proj-)?[A-Za-z0-9_-]{20,}",
        },
        PatternDef {
            name: "gcp_api_key",
            regex: r"AIza[0-9A-Za-z_-]{35}",
        },
        PatternDef {
            name: "gcp_service_acct",
            regex: r#""type"\s*:\s*"service_account""#,
        },
        PatternDef {
            name: "aws_access_key",
            regex: r"AKIA[0-9A-Z]{16}",
        },
        PatternDef {
            name: "aws_secret_key",
            regex: r"(?i)aws(.{0,20})?(secret|access).{0,20}?[\s:=]+[A-Za-z0-9/+=]{40}",
        },
        PatternDef {
            name: "github_token",
            regex: r"gh[pousr]_[A-Za-z0-9_]{30,}",
        },
        PatternDef {
            name: "github_classic",
            regex: r"(?i)github.{0,15}[\s:=]+[a-f0-9]{40}",
        },
        PatternDef {
            name: "databricks_token",
            regex: r"dapi[a-f0-9-]{32,}",
        },
        PatternDef {
            name: "huggingface_token",
            regex: r"hf_[A-Za-z0-9]{30,}",
        },
        PatternDef {
            name: "replicate_token",
            regex: r"r8_[A-Za-z0-9]{30,}",
        },
        PatternDef {
            name: "slack_token",
            regex: r"xox[abprs]-[A-Za-z0-9-]{10,}",
        },
        PatternDef {
            name: "jwt",
            regex: r"eyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}",
        },
        PatternDef {
            name: "bearer",
            regex: r"(?i)bearer\s+[A-Za-z0-9._\-]{20,}",
        },
        PatternDef {
            name: "pem_private_key",
            regex: r"-----BEGIN [A-Z ]*PRIVATE KEY-----[\s\S]*?-----END [A-Z ]*PRIVATE KEY-----",
        },
        PatternDef {
            name: "dotenv_secret",
            regex: r"(?im)^\s*[A-Z][A-Z0-9_]*(?:KEY|TOKEN|SECRET|PASSWORD|PASS|PWD|API)[A-Z0-9_]*\s*=\s*[^\s#\[][^\s#]{5,}",
        },
    ]
}
