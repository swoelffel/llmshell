# Audit log

LLMShell records every step of every session in an append-only, hash-chained, redacted audit log. This page describes its layout, the event taxonomy, the redaction model, and the invariants that make the log meaningful.

## Where it lives

By default, audit files are written under:

```
~/.llmsh/sessions/
```

One file per session, named after the session id, with the `.jsonl` extension. The directory is created with `0o700` permissions and audit files with `0o600`. Do not loosen those.

Override with the `audit.directory` field in the user config (`~/.config/llmsh/config.toml` on Linux, `~/Library/Application Support/llmsh/config.toml` on macOS — see [configuration.md](configuration.md) for all OSes). Disable entirely with `LLMSH_NO_AUDIT=1` (not recommended outside tests).

## Wire format

Each line is a `ChainedEvent` envelope: `schema_version`, `seq`, `prev_digest`, the event payload (flattened so `type` stays at the top level), and `digest`.

```json
{ "schema_version": 6, "seq": 12, "prev_digest": "…", "type": "tool_execution_start", "ts": "2026-05-08T10:21:33Z", "plan_id": "…", "step_id": "…", "tool": "read_file", "args_digest": "sha256:…", "digest": "…" }
```

- `schema_version` — current chain envelope version (v6).
- `seq` — monotonic per-session counter starting at 0.
- `prev_digest` — hex SHA-256 of the previous line's `digest`, or the session-seed digest for `seq == 0`.
- `type` — the variant tag (`#[serde(tag = "type", rename_all = "snake_case")]`).
- `ts` — RFC 3339 UTC timestamp.
- variant-specific fields.
- `digest` — hex SHA-256 over the canonical JSON of the envelope minus the `digest` field itself.

Schema version is exposed as `SCHEMA_VERSION` in [`crates/llmsh-audit/src/event.rs`](../crates/llmsh-audit/src/event.rs). Current value: `6`.

## Chain verification

```bash
llmsh verify-audit ~/.llmsh/sessions/<session>.jsonl
```

Or programmatically: `llmsh_audit::verify_chain(jsonl, session_id)` returns `VerifiedChain { events, sealed }`. A chain that ends in `session_ended` is **sealed**; one that doesn't is **unsealed** (truncated or writer crashed); any mismatch surfaces as a typed `ChainError` pointing to the first inconsistent line. v5 audit files remain readable as plain JSONL but cannot be chain-verified (verifier returns `SchemaTooOld`).

## Event taxonomy

| Variant | Emitted when |
|---|---|
| `SessionStarted` | A new REPL session begins. Carries `cwd`, `model`, `policy_mode`, `llmsh_version`, `schema_version`, `config_effective_hash`. |
| `UserInput` | The user submits a line. Text is redacted. |
| `LlmRequest` | A request is sent to the provider. Includes a digest of the messages and a redaction hit count. |
| `LlmResponse` | A response comes back. Carries finish reason, redacted message, tool-call digest, optional usage. |
| `ModelPlan` | The model proposes one or more tool calls in a turn. |
| `PolicyDecision` | `PolicyEngine` classifies a plan step. Records `effective_risk`, `action`, `flags`, `reasons`. |
| `ConfirmationAsked` | A `Confirm`-level action surfaces a prompt. Records whether it was granted. |
| `ToolExecutionStart` | A tool starts. Includes `args_digest` and a redacted preview. |
| `ToolExecutionEnd` | A tool finishes. Records `status`, `exit_code`, redacted stdout/stderr. |
| `RawShellExecution` | A `!`-prefixed raw shell line ran. |
| `AssistantMessage` | The assistant emits user-facing text. |
| `Error` | A failure occurred along any path. |
| `SessionEnded` | The REPL exits. |
| `MachineAuditPerformed` | An automated audit of the host machine ran (e.g. on `/init`). |
| `ModelChanged` | The active model changed via `/model`. Carries `from`, `to`. |
| `ContextCompacted` | `/compact` ran. Carries `reason`, `strategy`, `messages_before`/`_after`, `bytes_before`/`_after`, optional `summary_digest`. |
| `ContextCleared` | `/clear-context`, `/clear-memory`, `/clear-all`, or `/memory forget` ran. Carries `scope` (`context` / `memory` / `all` / `memory_forget`) and `rows_affected`. |
| `FactAdded` | A long-term fact was persisted. Carries `fact_id`, `category`, `source` (`manual` / `compact` / `init`). |
| `CwdChanged` | The working directory moved. Carries `from`, `to`, and `source` (`meta` / `raw_shell` / `tool`). |

## Hash chain

`AuditWriter` chains a hash from one line to the next: each event's `digest` is `sha256(prev_digest || line_bytes)`. Tampering with any line invalidates every subsequent line.

A direct `writeln!` to the JSONL bypasses the chain and is a regression. All audit writes must go through `AuditWriter`.

## Required invariants

These are non-negotiable (see [.claude/rules/audit-invariants.md](../.claude/rules/audit-invariants.md)):

1. **Every execution path emits an event on success, error, and cancellation.** A `?` propagation that skips an audit write is a regression.
2. **All audit writes go through `AuditWriter`.**
3. **Every text field carrying user, tool, or model output passes through `Redactor::default_audit()` before the write.**
4. **Tool outputs returned to the LLM pass through `llm_redact`** so secrets do not re-enter the conversation.
5. **Filesystem perms stay restrictive** — directory `0o700`, files `0o600`.

## Redaction

Two redactors operate at different boundaries:

- `Redactor::default_audit()` — strips secrets before they reach the audit file. Patterns include API keys (OpenAI, AWS, etc.), JWTs, common credential formats, file paths matching SSH key locations.
- `llm_redact` — strips secrets from tool output before it is sent back to the model in the next iteration. Prevents secret re-injection.

Both are best-effort string-level redactors. They are not a substitute for keeping secrets out of allowed roots.

## Adding a new event variant

1. Add fields needed to reconstruct what happened (timestamp, ids, redacted text fields).
2. Emit the event on the success **and** error **and** cancel paths of the new feature.
3. Mirror existing `#[serde(tag = "type", rename_all = "snake_case")]` conventions so JSONL substring asserts in tests keep working.
4. Bump `SCHEMA_VERSION` if existing fields change shape.
5. Write an e2e test under `crates/llmsh-core/tests/` asserting the event is emitted on the relevant paths.

## Reading the log

Each session file is plain JSONL. To inspect:

```bash
# Most recent session, pretty-printed
ls -t ~/.llmsh/sessions/ | head -1 | xargs -I{} cat ~/.llmsh/sessions/{} | jq .

# All policy decisions across all sessions
jq -c 'select(.type == "policy_decision")' ~/.llmsh/sessions/*.jsonl
```

To verify the chain (planned tooling on the [roadmap](../ROADMAP.md)):

```bash
# llmsh audit verify ~/.llmsh/sessions/<session>.jsonl
```
