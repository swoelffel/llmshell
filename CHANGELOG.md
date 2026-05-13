# Changelog

All notable changes to LLMShell are documented here. Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html) once it reaches v1.0.

## [0.3.0] — 2026-05-13

### Added

- **Anthropic provider** (`llmsh-llm-anthropic`) — native Claude Messages API
  implementation supporting Haiku (default), Sonnet, and Opus 4.x models.
  - Configured via `[providers.anthropic]` + `ANTHROPIC_API_KEY`.
  - Switch at runtime with `/provider set anthropic`; the provider's first
    model in `models[...]` (Haiku) is selected automatically.
  - JSON-object response_format is emulated server-side via the
    assistant-prefill `{` technique (cookbook) so the compactor keeps working
    transparently across providers.
  - HTTP error bodies are redacted via `llmsh-redact` (no `sk-ant-…` leaks).
- Pricing + context-window entries for `claude-haiku-4-5`,
  `claude-sonnet-4-6`, `claude-opus-4-7`.

### Not included (parked for a dedicated minor)

- Anthropic streaming (SSE) and explicit `cache_control: ephemeral` prompt
  caching — the system prompt is already ordered stable→dynamic, so the
  switch will be plumbing only.

## [0.2.15] — 2026-05-12

### Policy — pipeline-aware classifier

- `bash -c "A | B"`, `A && B`, `A || B` are now classified `ReadOnly`
  when every segment is independently read-only — the original user
  complaint `find . -maxdepth 1 -type f | wc -l` no longer triggers a
  confirmation prompt.
- Safe output redirections accepted: `>/dev/null`, `2>/dev/null`,
  `2>&1`, and `>/tmp/<simple-name>`. Any other target keeps the call at
  `Unknown` (confirm).
- Unsafe constructs still block classification: variable expansion
  (`$VAR`), command substitution (`$(…)`, backticks), globs, `;`
  sequence, `&` background, redirections to other paths.

### UX — explainable Unknown + light confirmation

- New `ClassificationReason` is propagated from the deterministic
  classifier into `PolicyDecision.classification_reason`. The
  confirmation prompt now reads `risk=Unknown — segment de pipeline
  non read-only` instead of an opaque `Unknown`.
- `PolicyAction::RequireConfirmation` gains `light: bool`. When the
  classifier returns `Unknown` but the model declared
  `claimed_risk = read_only|low`, the prompt is downgraded to a
  default-yes `[Y/n]` single-keystroke confirmation. The model never
  gains authority over execution — a confirm is still required, only
  the prompt is lighter.

### Persona — agent prompt updated

- System prompt now tells the model that simple pipelines and safe
  redirections in `bash -c "…"` are recognised by the classifier, so
  it can keep using legitimate shell forms without forcing the user
  through a confirmation.

## [0.2.14] — 2026-05-11

### Security — audit chain

- The audit JSONL is now tamper-evident: every line carries `seq`,
  `prev_digest`, and its own SHA-256 `digest` (envelope schema v6).
  The README's "SHA-256 chained" claim is now matched by the
  implementation.
- New `llmsh_audit::verify_chain` API and `llmsh verify-audit <path>`
  CLI subcommand walk the chain and report sealed / unsealed /
  broken-at-line outcomes.
- v5 audit files remain readable as plain JSONL but cannot be
  chain-verified (verifier returns `SchemaTooOld`).

## [0.2.13] — 2026-05-11

### Security — hardening pass

- New `llmsh-redact` crate centralises secret-pattern catalogue and engine.
  `llmsh-audit::redact` and `llmsh-core::llm_redact` are now thin façades
  over it, eliminating three previously parallel pattern lists.
- Extended pattern catalogue: OpenAI/Anthropic/GCP/AWS/GitHub/Databricks/
  HuggingFace/Replicate/Slack keys, JWT, Bearer tokens, PEM private keys,
  `.env`-style `*_KEY=…`/`*_PASSWORD=…` lines.
- OpenAI provider stores the API key inside `secrecy::SecretString`:
  no longer leaks in `Debug` output and is zeroed on drop.
- OpenAI HTTP error bodies pass through the redactor before being
  bubbled up to logs (some error responses echo request fragments).
- SQLite memory persistence redacts message content before insertion;
  previously `.env` reads or token-bearing tool outputs were stored
  verbatim.
- Policy `extract_shell_payload` accepts `bash -c PAYLOAD pos1 pos2…`
  (extra positional args after the payload), closing a gap where
  appending an argv tail let invocations skip read-only classification.

### Added

- Per-turn `SessionStats` tracked in the agent loop.
- `StatusPrompt` for the reedline status line.
- Tier-1 / tier-2 verbose output routed to stderr.
- `context_window` and pricing tables exposed on the LLM provider trait.
- `cached_input_tokens` surfaced from OpenAI usage payloads.

### Documentation

- Marketing storefront pass: README rewritten as a landing page; added `SECURITY.md`, `CONTRIBUTING.md`, `ROADMAP.md`, `CHANGELOG.md`; thematic docs under `docs/` (safety, audit, policy, configuration, examples); GitHub issue and PR templates.
- License aligned on **MIT only** (Cargo workspace metadata previously announced `MIT OR Apache-2.0`).
- Workspace `Cargo.toml` enriched with `description`, `repository`, `homepage`, `readme`, `keywords`, `categories`, `authors`; propagated to every crate manifest.

## [0.2.1] — 2026-05

### Added

- Confirmation prompt now shows resolved tool arguments and policy flags before execution.

## [0.2.0-context-memory] — 2026-04

### Added

- `SystemPromptBuilder` composing the per-turn system prompt as 5 ordered sections (persona, AGENTS.md, long-term memory, runtime context, recent activity). Stable→dynamic ordering is load-bearing for OpenAI's automatic prompt cache.
- `StaticSystemPrompt` and `MemorySystemPrompt` implementations of `SystemPromptSource`.
- AGENTS.md loader with a 2 KiB budget at `~/.config/llmsh/AGENTS.md`.
- SQLite-backed long-term memory layer (`Memory`, schema, migrations, CRUD), wired into `AgentDeps`.
- Recent-activity capture: user/assistant/tool actions recorded each turn, redacted before storage.
- `RuntimeContext` capture (cwd, OS, disk) using `sysinfo`.
- `MachineAudit` capture + markdown rendering.
- `/init` meta command that writes the initial audit to memory; auto-bootstrapped on first launch.
- `MachineAuditPerformed` and `ModelChanged` audit event variants.
- `LlmProvider::list_models` / `set_model` / `current_model`.
- `llmsh-llm-openai`: `/v1/models` discovery with chat-only filter and 60s TTL cache.
- Shared model state via `Arc<RwLock<String>>`, propagated through `AgentDeps` and persisted via `config::persist::set_default_model` (atomic, `toml_edit`).

### Fixed

- Atomic SQLite migrations (`IF NOT EXISTS` / `OR IGNORE`).
- Empty `LLMSH_MEMORY_DB` env var treated as unset.
- Recent-actions summary redacted before storage.
- TOCTOU race in `AGENTS.md` loader removed.
- `0600` permissions preserved when persisting `default_model`.
- `provider:model` prefix preserved through `/model set` persistence.
- Config parse errors now include the path.

## [0.1.0-slice] — 2026-03

Initial slice. Establishes the crate split and the core pipeline:

- `llmsh-llm`, `llmsh-llm-openai`, `llmsh-policy`, `llmsh-tools`, `llmsh-audit`, `llmsh-core`, `llmsh-cli`.
- OpenAI-compatible HTTP provider with neutral message/tool-call types.
- Policy engine returning `Allow` / `Confirm` / `Deny`.
- Built-in tools `read_file`, `list_directory`, `run_process`.
- Append-only JSONL audit log with hash-chained digests and redaction.
- Reedline-backed REPL with slash commands.
- Agent loop with bounded iterate-until-done, schema enrichment, sensitive-path checks.
- Confirmation gate trait with `AlwaysYesGate` / `AlwaysNoGate` test impls.
- Per-tool timeout and `CancellationToken` in the executor.

[Unreleased]: https://github.com/swoelffel/llmshell/compare/v0.2.1...HEAD
[0.2.1]: https://github.com/swoelffel/llmshell/releases/tag/v0.2.1
[0.2.0-context-memory]: https://github.com/swoelffel/llmshell/releases/tag/v0.2.0-context-memory
[0.1.0-slice]: https://github.com/swoelffel/llmshell/releases/tag/v0.1.0-slice
