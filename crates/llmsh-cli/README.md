# llmsh-cli

Binary entry-point for LLMShell. Parses CLI flags (`--config`, `--model`) and the `LLMSH_*` environment variables via `clap`, loads and merges user and project configs, initialises tracing, sets up the audit writer, builds the OpenAI provider, registers the built-in tools, constructs the policy engine, assembles `AgentDeps`, and hands off to the `Repl` in `llmsh-core`. This crate contains no domain logic of its own; its sole responsibility is bootstrapping all components and starting the interactive session.
