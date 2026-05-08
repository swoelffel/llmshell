---
paths:
  - "crates/llmsh-audit/**/*.rs"
  - "crates/llmsh-core/src/agent.rs"
  - "crates/llmsh-core/src/pipeline.rs"
  - "crates/llmsh-core/src/executor.rs"
  - "crates/llmsh-core/src/llm_redact.rs"
---

# Audit invariants

Canonical spec: [ai-docs/LLMShell_CDC_MVP.md](ai-docs/LLMShell_CDC_MVP.md).

## Required behaviour

- Every execution path emits an `AuditEvent` on **success, error, and cancellation**. A `?` propagation that skips an audit write is a regression.
- All audit writes go through `AuditWriter`, which chains a hash from one line to the next. A direct `writeln!` to the JSONL breaks that chain.
- Every text field carrying user, tool, or model output passes through `Redactor` (`Redactor::default_audit()`) before the write.
- Tool outputs returned to the LLM pass through `llm_redact` so sensitive content is not re-injected into the conversation.
- Filesystem perms: audit directory permissions are restrictive (octal seven-zero-zero), audit files restrictive (octal six-zero-zero), config restrictive (octal six-zero-zero). Do not loosen.

## Adding a new event variant

When introducing a new `AuditEvent` variant in [crates/llmsh-audit/src/event.rs](crates/llmsh-audit/src/event.rs):

1. Add fields needed to reconstruct what happened (timestamp, ids, redacted text fields).
2. Emit the event on the success **and** error **and** cancel paths of the new feature.
3. Mirror existing `#[serde(tag = "type", rename_all = "snake_case")]` conventions so JSONL substring asserts in tests keep working.

## Not a gate

This rule is guidance the model reads when editing audit-adjacent code. Hard guarantees come from `AuditWriter` itself plus the CI `cargo test` step.
