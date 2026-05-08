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

- **llmsh-llm** — `LlmProvider` trait, message/tool-call types, `Capabilities` (tool-calling mode, JSON mode, parallel calls).
- **llmsh-llm-openai** — OpenAI-compatible HTTP impl. Mapping between internal types and wire format lives in `mapping.rs` / `wire.rs`; keep these symmetric.
- **llmsh-policy** — `PolicyEngine` classifies each tool call into a `RiskLevel` and returns a `RiskAction` (`Allow` / `Confirm` / `Deny`). `phrase.rs` and `sensitive.rs` drive heuristics; `paths.rs` resolves filesystem scope against allowed roots.
- **llmsh-tools** — Built-in tools (`read_file`, `list_directory`, `run_process`) behind a `Tool` trait, exposed through a `ToolRegistry`. `enrich.rs` adds JSON-schema enrichment that the LLM consumes.
- **llmsh-audit** — Append-only newline-JSON audit writer with hash-chained `digest`, redaction (`redact.rs`), session ids, and event taxonomy (`event.rs`). Treat the chain as load-bearing — never mutate a written line.
- **llmsh-core** — Integration hub. Key pieces:
  - `agent::AgentLoop` — the iterate-until-done loop: build context → call provider → if tool calls, run them through the pipeline; bounded by `AgentBounds` (`max_iterations`, `max_tool_calls_per_iteration`, `max_schema_repair_attempts`).
  - `pipeline::Pipeline` — schema enrichment + policy classification + sensitive-path checks before a tool runs.
  - `executor::ToolExecutor` — runs tools with per-tool timeout and a `CancellationToken`.
  - `confirm::ConfirmationGate` — trait used to prompt for `Confirm`-level actions; tests use `AlwaysYesGate`/`AlwaysNoGate`.
  - `repl::Repl` — reedline-backed input, slash commands, session state.
  - `config/` — TOML loader with user + project merge.
  - `context.rs` — `SystemPromptBuilder` composes the per-turn system prompt as 5 ordered sections (persona, AGENTS.md, long-term memory, runtime context, recent activity). `SystemPromptSource` trait + `StaticSystemPrompt` / `MemorySystemPrompt` impls. **Stable→dynamic ordering is load-bearing for OpenAI's automatic prompt cache — don't reorder.**
  - `llm_redact.rs`, `raw_shell.rs` — redaction at the LLM boundary, raw-shell risk scan.
- **llmsh-cli** — `clap`/`tokio` entry point that constructs every dependency and starts the REPL.

### Request flow

`Repl` reads a line → `AgentLoop::run` appends to `ContextBuilder`, requests completion from the `LlmProvider` with the registry's tool specs → on `tool_calls`, each goes through `Pipeline` (schema check, `PolicyEngine` classification, sensitive-path gate) → if `Confirm`, the `ConfirmationGate` prompts the user → `ToolExecutor` runs it with timeout/cancellation → result is appended to context and the loop continues until the model finishes or `max_iterations` is hit. Every decision and result is written via `AuditWriter` with redaction applied first.

### Testing patterns

Integration tests live in [crates/llmsh-core/tests/](crates/llmsh-core/tests/) and use [tests/common/mod.rs](crates/llmsh-core/tests/common/mod.rs), which provides `MockLlmProvider` (scripted responses) and `build_test_deps`. When adding agent-loop behavior, prefer an `e2e_*.rs` test that scripts provider responses end-to-end over a unit test of the loop internals — that's the established pattern and covers redaction, policy, repair, and cancellation.

## Conventions

- Workspace deps are pinned in the root [Cargo.toml](Cargo.toml) under `[workspace.dependencies]`; reference them in member crates with `dep.workspace = true` rather than re-pinning versions.
- `reqwest` uses `rustls-tls` with `default-features = false` — don't reintroduce `native-tls`.
- Audit events are the source of truth for "what happened"; if you add a new agent action, add a corresponding `AuditEvent` variant and emit it on every path (including error/cancel).
- The MVP brief at [ai-docs/briefs/archived/LLMShell_CDC_MVP.md](ai-docs/briefs/archived/LLMShell_CDC_MVP.md) is the canonical spec for risk levels, redaction rules, and confirmation semantics — consult it before changing policy or audit behavior.
