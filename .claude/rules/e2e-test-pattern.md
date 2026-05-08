---
paths:
  - "crates/llmsh-core/tests/**/*.rs"
---

# E2E test pattern

Integration tests for the agent loop live in `crates/llmsh-core/tests/`. The pattern below is the project convention — match it instead of innovating.

## File layout

- File name: `e2e_<snake_case>.rs`. Match the naming of existing siblings.
- Start with `mod common;` and reuse `common::MockLlmProvider` plus `common::build_simple_deps` (default) or `common::build_test_deps` (custom gate / sensitive patterns / cwd) from [crates/llmsh-core/tests/common/mod.rs](crates/llmsh-core/tests/common/mod.rs).
- Use `#[tokio::test]`.

## Scenario shape

1. Build a `tempfile::tempdir()` for the workspace and a separate one for the audit dir. **Canonicalise** paths on macOS — the `/var → /private/var` symlink will otherwise cause spurious "outside_workspace" denials. See the comment in [crates/llmsh-core/tests/e2e_redaction.rs](crates/llmsh-core/tests/e2e_redaction.rs).
2. Build a `Vec<LlmResponse>` consumed in order. Typically a tool-calling response (`finish_reason: ToolCalls`) followed by a stop response (`finish_reason: Stop`). Always end with `Stop` unless explicitly testing iteration limits.
3. Tool-call args are `serde_json::json!(...)`.
4. Construct an `AgentLoop { deps, builder: ContextBuilder::new(...) }` and call `agent.run("…").await`.

## Assertions

- Flush the audit writer (`deps.audit.lock().unwrap().flush().unwrap();`) and read `audit_dir.path().join("test-session.jsonl")` as a string.
- `AuditEvent` serialises with `#[serde(tag = "type", rename_all = "snake_case")]`. Grep the JSONL for substrings:
  - Event types: `"type":"tool_execution_end"`, `"type":"policy_decision"`, `"type":"error"`, …
  - Field values: `"action":"deny"`, `"effective_risk":"strong"`.
  - Redaction markers: `[REDACTED:<kind>]`.
- Avoid full JSON parsing unless an assertion truly needs it.

## Fixed-string fakes

Negative-assertion tests (e.g. `e2e_redaction.rs`) compare the audit log against literal placeholder constants defined as `const FAKE_*` at the top of the test file. Reuse the existing constants when possible. If you need new ones, define them as `const`s in the test file rather than pasting raw key-shaped strings into prompts.

## Constraints

- Don't touch `tests/common/mod.rs` unless the scenario genuinely needs new shared infra. If you do, mark new helpers with `#[allow(dead_code)]` and document them inline like the existing helpers.
- Don't add new workspace dependencies. The current set (`tokio`, `serde_json`, `tempfile`, `tokio-util`, etc.) is enough.
- Run with `cargo test -p llmsh-core --test e2e_<name>` (use `--no-run` for compile-only). A failing test means production code or the test is wrong — diagnose, don't paper over.
