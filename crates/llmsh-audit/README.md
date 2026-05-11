# llmsh-audit

Structured, tamper-evident audit log for LLMShell. Writes newline-delimited JSON records (`AuditEvent`) to a per-session log file, covering session lifecycle, every LLM request and response, each tool invocation and its output, and user confirmations. A `Redactor` strips secrets and sensitive values from tool outputs before they are persisted, keeping audit files safe to store and share.

## Chain verification

Each line is wrapped in a `ChainedEvent` envelope: `schema_version`, `seq`, `prev_digest` (hex SHA-256 of the previous line's `digest`, or a session-seed hash for the first line), the event payload, and `digest` (hex SHA-256 over the envelope minus the `digest` field). A chain is verifiable via `llmsh_audit::verify_chain(jsonl, session_id)` or from the command line: `llmsh verify-audit ~/.llmsh/sessions/<session>.jsonl`. A chain that ends in `session_ended` is reported as **sealed**; one that doesn't is **unsealed** (truncated or writer crashed); any mismatch surfaces as a typed `ChainError` pointing to the first inconsistent line.
