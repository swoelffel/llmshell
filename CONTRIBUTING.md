# Contributing to LLMShell

Thanks for your interest in LLMShell! This document covers the practical bits of contributing code.

## Development setup

LLMShell is a Rust workspace, edition 2021. The toolchain is pinned by [`rust-toolchain.toml`](rust-toolchain.toml) (stable + `rustfmt` + `clippy`, MSRV `1.78`).

```bash
git clone https://github.com/swoelffel/llmshell
cd llmshell
cargo build --release
```

You will need an OpenAI-compatible API key to exercise the agent loop end-to-end:

```bash
export OPENAI_API_KEY=sk-...
./target/release/llmsh
```

## Build, test, lint

CI runs the same three gates locally and on push. Match them before opening a PR:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
```

Useful narrow-scope variants:

```bash
cargo test -p llmsh-core --test e2e_redaction          # one integration test file
cargo test -p llmsh-core e2e_redaction::test_x         # one test by name
cargo build --release                                  # binary at target/release/llmsh
```

## Crate architecture

Seven crates wired together by `llmsh-core` and bootstrapped by `llmsh-cli`:

- `llmsh-llm` — provider-neutral LLM trait + neutral message/tool-call types.
- `llmsh-llm-openai` — OpenAI-compatible HTTP provider.
- `llmsh-policy` — `RiskAction` (`Allow` / `Confirm` / `Deny`) classifier.
- `llmsh-tools` — `read_file`, `list_directory`, `run_process` behind a `Tool` trait.
- `llmsh-audit` — append-only JSONL with hash-chained `digest`, redaction, event taxonomy.
- `llmsh-core` — agent loop, pipeline, executor, REPL, confirmation gate, system-prompt builder.
- `llmsh-cli` — `clap`/`tokio` entry point.

Path-scoped conventions live in [`.claude/rules/`](.claude/rules/) and load only when relevant files enter context. The most important ones:

- [`.claude/rules/audit-invariants.md`](.claude/rules/audit-invariants.md) — invariants for the audit log.
- [`.claude/rules/policy-rules.md`](.claude/rules/policy-rules.md) — invariants for the policy engine.
- [`.claude/rules/e2e-test-pattern.md`](.claude/rules/e2e-test-pattern.md) — pattern for end-to-end tests under `crates/llmsh-core/tests/`.

## Security expectations

LLMShell is a security-sensitive codebase. Two non-negotiables:

1. **Never bypass the policy gate or the audit chain.** Every tool execution must go through the pipeline. If you need to add a new tool, register it via `ToolRegistry`, write its policy classification, and emit audit events on the same code paths as existing tools.
2. **Never widen the redactor's blind spots.** The redactor operates at the LLM boundary; if you add a new field that may carry secrets, ensure it goes through `redact.rs` before being logged or sent to a provider.

Contributions that touch `llmsh-policy/`, `llmsh-audit/`, or the gating code in `llmsh-core/{pipeline,agent,executor,confirm,raw_shell,llm_redact}.rs` should explicitly call out the security impact in the PR.

## Pull request checklist

Before requesting review:

- [ ] `cargo fmt --all -- --check` passes.
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` passes.
- [ ] `cargo test --workspace --locked` passes.
- [ ] New behaviour has a test (unit or e2e under `crates/llmsh-core/tests/`).
- [ ] Audit / policy impact is described in the PR body.
- [ ] User-visible changes have a `CHANGELOG.md` entry under `[Unreleased]`.

## Reporting bugs and proposing features

- Bugs: use the **Bug report** issue template.
- Features: use the **Feature request** issue template — describe the *risk/safety impact*, not just the user-facing change.
- Security issues: see [SECURITY.md](SECURITY.md). Please do not open public issues for vulnerabilities.

## Code of conduct

Be kind, be precise, prefer evidence over assertions. PR reviews focus on the code and the design, not the contributor.
