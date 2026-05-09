# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project docs layout (override for superpowers)

`docs/` is the public-facing documentation and must not be polluted by per-iteration briefs/plans/specs. All draft artefacts live under `ai-docs/` (gitignored).

```
ai-docs/
├── ROADMAP.md             ← single source of truth for status
├── current/               ← work-in-progress (briefs, plans, specs in flight)
└── releases/
    └── vX.Y.Z-<slug>/     ← brief.md / spec.md / plan.md, grouped per shipped release
```

**Conventions:**
- New work (brainstorming, plans, briefs) lands flat in `ai-docs/current/`. Date-prefix optional; the file name should describe the topic.
- When a release ships, move its artefacts into `ai-docs/releases/vX.Y.Z-<slug>/` and rename to `brief.md`, `spec.md`, `plan.md` (additional companion docs allowed, e.g. `brief-explained.md`).
- "Active vs archived" is read from `ROADMAP.md`, not from directory names.

**Superpowers skill overrides** (precedence over the skills' own defaults, per the using-superpowers contract):

| Skill | Default path | Use here instead |
|---|---|---|
| `superpowers:writing-plans` | `docs/superpowers/plans/YYYY-MM-DD-*.md` | `ai-docs/current/YYYY-MM-DD-*-plan.md` |
| `superpowers:brainstorming` (design docs) | `docs/superpowers/specs/YYYY-MM-DD-*-design.md` | `ai-docs/current/YYYY-MM-DD-*-design.md` |

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

Useful env vars: `LLMSH_DEBUG=1` (tracing to stderr), `LLMSH_VERBOSE=1|2` (per-turn stats; CLI `-v` / `-vv` are equivalents), `LLMSH_NO_AUDIT=1` (disable audit — tests rely on this off), `LLMSH_NO_AUTOINIT=1` (skip the bootstrap `/init`), `LLMSH_CONFIG`, `LLMSH_MODEL`, `LLMSH_MEMORY_DB`. CLI flags: `-v` / `-vv`, `--config <path>`. First launch writes `~/.config/llmsh/config.toml`; a `.llmsh.toml` in the cwd merges on top. Audit log: `~/.llmsh/sessions/` (override via `audit.directory`).

## Architecture

Seven crates in [crates/](crates/), wired together by [llmsh-core](crates/llmsh-core/) and bootstrapped by [llmsh-cli/src/main.rs](crates/llmsh-cli/src/main.rs):

- **llmsh-llm** — `LlmProvider` trait, neutral message/tool-call types, `Capabilities`.
- **llmsh-llm-openai** — OpenAI-compatible HTTP impl. Mapping in `mapping.rs` / `wire.rs`.
- **llmsh-policy** — `PolicyEngine` returns `RiskAction` (`Allow` / `Confirm` / `ConfirmStrong` / `Deny`). The "workspace boundary" enforcement was dropped in v0.2.7: `allowed_roots` from the user config is still surfaced via `PolicyContext` as a best-effort hint to the agent, but is no longer enforced — the host filesystem (scoped by the running user's rights) is the boundary. See [.claude/rules/policy-rules.md](.claude/rules/policy-rules.md).
- **llmsh-tools** — `read_file`, `list_directory`, `run_process`, `glob` behind a `Tool` trait + `ToolRegistry`. `read_file` / `list_directory` / `run_process` expand `~` / `~/…` to `$HOME` (no shell, no glob expansion); use `glob` first when patterns are needed.
- **llmsh-audit** — append-only JSONL with hash-chained `digest`, redactor, session ids, event taxonomy. See [.claude/rules/audit-invariants.md](.claude/rules/audit-invariants.md).
- **llmsh-core** — integration hub:
  - `agent::AgentLoop` — bounded iterate-until-done loop (`AgentBounds`).
  - `pipeline::Pipeline` — schema enrichment + policy classification + sensitive-path checks.
  - `executor::ToolExecutor` — per-tool timeout + `CancellationToken`. PWD is shared across REPL/policy/tools via `cwd::SharedCwd = Arc<RwLock<PathBuf>>`; `!cd /dir` and `run_process(cd, ["…"])` mutate it (and emit a `CwdChanged` audit event).
  - `confirm::ConfirmationGate` — trait used to prompt for `Confirm` and `ConfirmStrong` actions (the latter requires typing a generated phrase verbatim).
  - `repl::Repl` — reedline-backed input + slash commands.
  - `memory::Memory::cleanup_orphan_tool_calls` — one-shot startup pass that drops any persisted assistant `tool_calls` left without matching tool responses (a legacy v0.2.6 DB would otherwise cause OpenAI to 400 on session reload).
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
