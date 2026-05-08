# llmsh-audit

Structured, tamper-evident audit log for LLMShell. Writes newline-delimited JSON records (`AuditEvent`) to a per-session log file, covering session lifecycle, every LLM request and response, each tool invocation and its output, and user confirmations. Events are SHA-256 chained so the log can be verified for completeness. A `Redactor` strips secrets and sensitive values from tool outputs before they are persisted, keeping audit files safe to store and share.
