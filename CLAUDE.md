# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project

LLMShell (`llmsh`) — an agentic terminal shell. A REPL takes natural-language input, an LLM agent plans and emits tool calls, a policy engine classifies risk and gates execution, tools run with timeout/cancellation, and every step is appended to a tamper-evident audit log.

## Build / test / lint

Rust workspace, edition 2021, MSRV 1.78 (pinned via [rust-toolchain.toml](rust-toolchain.toml) — stable + rustfmt + clippy).

```bash
cargo build --release                           # binary at target/release/llmsh
cargo test --workspace --locked                 # full test suite (CI uses --locked)
cargo test -p llmsh-core --test e2e_redaction   # one integration test file
cargo test -p llmsh-core e2e_redaction::test_x  # one test by name
cargo fmt --all -- --check                      # CI gate
cargo clippy --workspace --all-targets -- -D warnings   # CI gate (warnings = failures)
```

CI ([.github/workflows/ci.yml](.github/workflows/ci.yml)) runs fmt + clippy + tests on Linux and tests on macOS. Match it locally before pushing.

## Run

```bash
export OPENAI_API_KEY=sk-...
./target/release/llmsh                # or: cargo run -p llmsh-cli
```

Useful env vars: `LLMSH_DEBUG=1` (tracing to stderr), `LLMSH_NO_AUDIT=1` (disable audit — tests rely on this off), `LLMSH_CONFIG`, `LLMSH_MODEL`. First launch writes `~/.config/llmsh/config.toml`; a `.llmsh.toml` in the cwd merges on top. Audit log: `~/.local/share/llmsh/audit/`.

## Architecture

Seven crates in [crates/](crates/), wired together by [llmsh-core](crates/llmsh-core/) and bootstrapped by [llmsh-cli/src/main.rs](crates/llmsh-cli/src/main.rs):

- **llmsh-llm** — `LlmProvider` trait, neutral message/tool-call types, `Capabilities`.
- **llmsh-llm-openai** — OpenAI-compatible HTTP impl. Mapping in `mapping.rs` / `wire.rs`.
- **llmsh-policy** — `PolicyEngine` returns `RiskAction` (`Allow` / `Confirm` / `Deny`). See [.claude/rules/policy-rules.md](.claude/rules/policy-rules.md).
- **llmsh-tools** — `read_file`, `list_directory`, `run_process` behind a `Tool` trait + `ToolRegistry`.
- **llmsh-audit** — append-only JSONL with hash-chained `digest`, redactor, session ids, event taxonomy. See [.claude/rules/audit-invariants.md](.claude/rules/audit-invariants.md).
- **llmsh-core** — integration hub:
  - `agent::AgentLoop` — bounded iterate-until-done loop (`AgentBounds`).
  - `pipeline::Pipeline` — schema enrichment + policy classification + sensitive-path checks.
  - `executor::ToolExecutor` — per-tool timeout + `CancellationToken`.
  - `confirm::ConfirmationGate` — trait used to prompt for `Confirm`-level actions.
  - `repl::Repl` — reedline-backed input + slash commands.
  - `context.rs` — `SystemPromptBuilder` composes the per-turn system prompt as 5 ordered sections (persona, AGENTS.md, long-term memory, runtime context, recent activity). `SystemPromptSource` trait + `StaticSystemPrompt` / `MemorySystemPrompt` impls. **Stable→dynamic ordering is load-bearing for OpenAI's automatic prompt cache — don't reorder.**
  - `llm_redact.rs`, `raw_shell.rs` — redaction at the LLM boundary, raw-shell risk scan.
- **llmsh-cli** — `clap`/`tokio` entry point.

### Request flow

`Repl` reads a line → `AgentLoop::run` builds context, calls the provider with the registry's tool specs → tool calls go through `Pipeline` (schema, policy, sensitive paths) → `Confirm` → `ConfirmationGate` → `ToolExecutor` runs with timeout/cancel → result feeds the next iteration until `Stop` or `max_iterations`. Every decision and result lands in the audit log via `AuditWriter` after redaction.

## Path-scoped rules

Conventions for specific areas live in [.claude/rules/](.claude/rules/) and load only when a matching file enters context:

- [audit-invariants.md](.claude/rules/audit-invariants.md) — touched when editing `llmsh-audit` or audit-emitting code in `llmsh-core`.
- [policy-rules.md](.claude/rules/policy-rules.md) — touched when editing `llmsh-policy`, `pipeline.rs`, `confirm.rs`, `raw_shell.rs`.
- [e2e-test-pattern.md](.claude/rules/e2e-test-pattern.md) — touched when editing files under `crates/llmsh-core/tests/`.

## Reference

- Workspace deps pinned in [Cargo.toml](Cargo.toml) under `[workspace.dependencies]`; reference with `dep.workspace = true`.
- `reqwest` uses `rustls-tls` (`default-features = false`) — don't reintroduce `native-tls`.
- MVP brief (canonical for risk levels, redaction rules, confirmation semantics): [ai-docs/briefs/archived/LLMShell_CDC_MVP.md](ai-docs/briefs/archived/LLMShell_CDC_MVP.md).
