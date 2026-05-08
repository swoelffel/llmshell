# LLMShell

LLMShell (`llmsh`) is an agentic shell that replaces your terminal prompt with an LLM-powered agent. Type a natural-language task; the agent plans, calls tools (read files, list directories, run processes), enforces a configurable risk policy before every action, and writes a tamper-evident audit log — all within your current working directory.

## Project layout

```
crates/
  llmsh-llm          # Provider trait + shared message types
  llmsh-llm-openai   # OpenAI-compatible HTTP provider
  llmsh-policy       # Risk-classification and enforcement engine
  llmsh-tools        # Built-in tool implementations
  llmsh-audit        # Structured audit-log writer
  llmsh-core         # Agent loop, REPL, config, pipeline orchestration
  llmsh-cli          # Binary entry-point (`llmsh`)
```

## Requirements

- Rust 1.78 or later (see `rust-toolchain.toml`)
- An OpenAI-compatible API key (set `OPENAI_API_KEY`)

## Build

```bash
cargo build --release
```

The binary lands at `target/release/llmsh`.

## Run

```bash
export OPENAI_API_KEY=sk-...
./target/release/llmsh
```

On first run a default config is created at `~/.config/llmsh/config.toml`. You can override it with `--config <path>` or select a model with `--model provider:model-name`.

## Configuration

`~/.config/llmsh/config.toml` controls the default model, per-risk-level policy actions (allow / confirm / deny), filesystem allowed roots, tool timeouts, and audit log directory. A project-level `.llmsh.toml` in the current directory merges on top of the user config.

## Debug / audit

Set `LLMSH_DEBUG=1` to emit tracing output on stderr.  
Set `LLMSH_NO_AUDIT=1` to disable the audit log (not recommended in production).  
Audit events are written as newline-delimited JSON to `~/.local/share/llmsh/audit/`.

## License

Dual-licensed under **MIT OR Apache-2.0**. See [LICENSE](LICENSE).
