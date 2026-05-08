# Roadmap

LLMShell is pre-1.0 and shipped as experimental software. The roadmap below sketches the next milestones; scope and ordering are subject to change as we get feedback.

## v0.3 — Install & documentation

Goal: drop the time-to-first-session below two minutes.

- Pre-built release binaries for `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`, `aarch64-apple-darwin`.
- `SHA256SUMS` published with each release.
- `install.sh` script (`curl -fsSL … | sh`) that detects OS/arch, downloads the matching binary, verifies the checksum, installs to `~/.local/bin`.
- Homebrew tap: `brew install swoelffel/tap/llmshell`.
- `cargo install --git` documented as the official source path until crates.io publication.
- README demo (asciinema or GIF) above the fold.
- `docs/safety.md` finalised and linked from the safety section.

## v0.4 — Local-first provider

Goal: usable offline / on a laptop without sending data to a hosted provider.

- Ollama provider behind the `LlmProvider` trait.
- Provider diagnostics: `llmsh /doctor` reports reachability, model availability, capability flags.
- Capability detection so the agent loop adapts to providers without parallel tool calls or JSON mode.

## v0.5 — Developer workflows

Goal: useful daily-driver for code-aware tasks.

- `git_status`, `git_diff` typed tools.
- Project-aware context (detect language, build system, test command).
- Slash commands for common workflows (`/test`, `/lint`, `/explain`).

## v0.6 — Controlled write operations

Goal: safe write tools without surprises.

- `write_file`, `edit_file` typed tools.
- Preview / dry-run mode that shows the diff before applying.
- Stronger confirmation UX for write operations.

## v0.7 — Provider compatibility

Goal: work with non-OpenAI providers that don't expose tool-calling natively.

- JSON-fallback provider (model returns structured tool calls in plain JSON).
- Capability-driven selection in the agent loop.
- Tool-call eval suite to compare providers on the same benchmarks.

## Beyond v0.7

These are tracked but unscheduled:

- crates.io publication.
- Sandboxed execution (Linux: bubblewrap / seccomp; macOS: sandbox-exec) — would replace "not a sandbox" caveats in the safety model.
- Multi-session memory across runs.
- Plugin tools loaded at runtime (out-of-tree).

## Phase 5 launch (post-v0.3)

Once the install path is smooth and the demo asset exists, run a controlled launch on Show HN, Lobsters, Reddit, LinkedIn. Until then, the project is intentionally low-profile.
