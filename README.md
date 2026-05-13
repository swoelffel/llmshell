# LLMShell

**Safety-first agentic shell for developers and operators.**

![CI](https://github.com/swoelffel/llmshell/actions/workflows/ci.yml/badge.svg)
![License](https://img.shields.io/badge/license-MIT-blue)
![Rust](https://img.shields.io/badge/rust-1.78%2B-orange)
![Status](https://img.shields.io/badge/status-experimental-yellow)

LLMShell (`llmsh`) lets you describe terminal tasks in natural language. The agent plans, calls typed tools, checks a configurable policy before risky actions, and records a redacted audit trail.

> The AI shell that asks before it acts — and records what it did.

## Quick start

```bash
cargo install --git https://github.com/swoelffel/llmshell --locked
export OPENAI_API_KEY=sk-...
llmsh
```

On first launch, `llmsh` writes a default user config (`~/.config/llmsh/config.toml` on Linux, `~/Library/Application Support/llmsh/config.toml` on macOS, `%APPDATA%\llmsh\config.toml` on Windows). A project-level `.llmsh.toml` in the current directory merges on top. See [docs/configuration.md](docs/configuration.md).

## Example session

```text
llmsh> list the files in this directory
[tool] list_directory
[assistant] Cargo.toml, README.md, crates/, …

llmsh> read README.md and summarise it
[tool] read_file
[assistant] LLMShell is a safety-first agentic shell …

llmsh> read ~/.ssh/id_rsa
[policy] confirm (strong): sensitive path
  flags:   [SensitivePath]
  reason:  matches built-in sensitive_paths pattern "~/.ssh/**"
To confirm, type:  read id_rsa
> ^C
[policy] cancelled

llmsh> !ls -la
[raw shell] executed and audited
```

Most AI terminal tools focus on generating commands. LLMShell focuses on **controlled execution**.

## Why LLMShell?

- **Typed tools, not raw shell by default** — the agent calls `read_file`, `list_directory`, `run_process`, `glob` with structured arguments, not free-form commands.
- **Policy gate before every action** — each tool call is classified into `Allow` / `Confirm` / `ConfirmStrong` / `Deny` before it runs.
- **Sensitive path detection** — paths like `~/.ssh/`, credentials files, and well-known system locations require **strong confirmation** (typing a generated phrase) by default. Users can map them to `Deny` in `config.toml`.
- **Confirmation prompts on risky operations** — destructive or ambiguous calls surface tool args + policy flags before execution.
- **Smart confirmation prompts** — read-only `run_process` invocations (`crontab -l`, `git status`, `ls`, …) are auto-classified and pass without prompting. The model also declares an `intent` and a `claimed_risk` for every `run_process` call, both surfaced in the confirmation prompt and audit log. The model can only RAISE risk above the deterministic verdict, never lower it.
- **Redacted, append-only audit log** — every step is recorded as hash-chained JSONL with secrets stripped at the LLM boundary.
- **Explicit raw shell escape via `!`** — when you really need raw shell, prefix with `!`. It still goes through the audit log.

## How it compares

| Category | Main focus | LLMShell difference |
|---|---|---|
| Command generators | Generate shell commands from prompts | LLMShell executes typed tools through a policy engine |
| Terminal agents | Let an LLM operate in a terminal | LLMShell emphasises policy, audit and controlled execution |
| AI terminals | Improve terminal UX with AI | LLMShell focuses on the shell/runtime layer |
| Natural-language shells | Interpret natural language as actions | LLMShell is safety-first and audit-first |

## Safety model

- The LLM proposes; the runtime decides.
- The `ToolRegistry` is the only source of executable tools.
- Sensitive paths require strong confirmation by default (configurable to deny).
- Risky actions require explicit confirmation.
- The audit log is local, redacted, and append-only.
- LLMShell is **not** a sandbox — it adds gates around tool calls, not OS-level isolation.

Full details: [docs/safety.md](docs/safety.md).

## Architecture

Eight Rust crates:

- `llmsh-llm` — provider-neutral `LlmProvider` trait + neutral message/tool-call types.
- `llmsh-llm-openai` — OpenAI-compatible HTTP provider.
- `llmsh-llm-anthropic` — Anthropic Messages API provider (Claude Haiku / Sonnet / Opus).
- `llmsh-llm-ollama` — local Ollama provider.
- `llmsh-policy` — `RiskAction` (`Allow` / `Confirm` / `ConfirmStrong` / `Deny`) classifier.
- `llmsh-tools` — `read_file`, `list_directory`, `run_process`, `glob` behind a `Tool` trait.
- `llmsh-audit` — append-only JSONL with hash-chained `digest`, redaction, event taxonomy.
- `llmsh-core` — agent loop, pipeline (schema + policy + sensitive paths), executor, REPL, confirmation gate.
- `llmsh-cli` — `clap`/`tokio` entry point, builds the `llmsh` binary.

## Installation

### From source (recommended for now)

```bash
cargo install --git https://github.com/swoelffel/llmshell --locked
```

### Build for development

```bash
git clone https://github.com/swoelffel/llmshell
cd llmshell
cargo build --release
./target/release/llmsh
```

### Pre-built binaries

Pre-built Linux/macOS binaries, an `install.sh` script and a Homebrew tap are tracked on the [roadmap](ROADMAP.md) for v0.3.

### Reinstalling after a rebuild (macOS gotcha)

When upgrading an existing install, prefer `cargo install --git … --force` or `cargo install --path crates/llmsh-cli --force` rather than `cp target/release/llmsh ~/.cargo/bin/llmsh`. On macOS Sequoia, overwriting the binary in place can keep a cached TCC / launch-services verdict bound to the previous inode — the new binary then dies silently with `zsh: killed llmsh`. If you hit it, the unstick is one line:

```bash
rm ~/.cargo/bin/llmsh && cp target/release/llmsh ~/.cargo/bin/llmsh
```

The freshly-created inode resets the cached verdict.

## Configuration

The user `config.toml` (location depends on OS — see [docs/configuration.md](docs/configuration.md)) controls:

- default model (`provider:model-name`),
- per-risk-level policy actions (allow / confirm / deny),
- filesystem allowed roots,
- per-tool timeouts,
- audit log directory.

A project-level `.llmsh.toml` merges on top of the user config.

Useful environment variables:

- `OPENAI_API_KEY` — required.
- `LLMSH_MODEL` — override default model for a session.
- `LLMSH_CONFIG` — alternative config path.
- `LLMSH_DEBUG=1` — tracing on stderr.
- `LLMSH_NO_AUDIT=1` — disable the audit log (not recommended).

Full reference: [docs/configuration.md](docs/configuration.md).

## Status

LLMShell is **early-stage experimental software**. Do not use it on production systems or sensitive environments without reviewing the policy configuration first.

Current capabilities:

- providers: OpenAI-compatible, Anthropic (Claude 4.x), Ollama — with runtime model switch (`/model`) and provider switch (`/provider set <name>`),
- natural-language REPL with slash commands,
- typed tools: `list_directory`, `read_file`, `run_process`, `glob`,
- policy engine with sensitive-path protection,
- raw shell escape via `!`,
- redacted JSONL audit log with hash chain,
- Linux and macOS development targets.

## Roadmap

See [ROADMAP.md](ROADMAP.md). Highlights for v0.3: release binaries, install script, Homebrew tap, demo asciinema.

## Contributing

Contributions welcome — see [CONTRIBUTING.md](CONTRIBUTING.md). Security issues: please follow [SECURITY.md](SECURITY.md).

## License

MIT. See [LICENSE](LICENSE).
