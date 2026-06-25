# Configuration

LLMShell merges three layers, each overriding the previous:

1. Built-in defaults.
2. User config (path depends on OS, see below).
3. Project config: `.llmsh.toml` in the current working directory (optional).

The first launch writes a default user config; missing files at lower layers are silently ignored.

## File locations

User config and the memory database live in the OS-specific application support directory (resolved via the `directories` crate):

| OS | User config | Memory store |
|---|---|---|
| Linux | `~/.config/llmsh/config.toml` (or `$XDG_CONFIG_HOME/llmsh/config.toml`) | `~/.local/share/llmsh/memory.db` |
| macOS | `~/Library/Application Support/llmsh/config.toml` | `~/Library/Application Support/llmsh/memory.db` |
| Windows | `%APPDATA%\llmsh\config.toml` | `%APPDATA%\llmsh\memory.db` |

The rest of the paths are platform-uniform:

| Path | Purpose |
|---|---|
| `<config-dir>/AGENTS.md` | User-level agent instructions, loaded into the system prompt with a 2 KiB budget. The `<config-dir>` is the same directory as `config.toml` above. |
| `.llmsh.toml` (cwd) | Project overrides. Merged on top of the user config. |
| `~/.llmsh/sessions/` | Audit log directory (mode `0o700`). Override with `audit.directory` in `config.toml`. |

User config file mode: `0o600`. Throughout the rest of this document, `~/.config/llmsh/config.toml` is used as a shorthand — substitute the macOS or Windows path as appropriate for your system.

Override the config path with `--config <path>` or `LLMSH_CONFIG`.

## Sample `config.toml`

```toml
# Default model. Format: "<provider>:<model-name>".
default_model = "openai:gpt-4o-mini"

[providers.openai]
# Optional. Defaults to OpenAI's hosted endpoint.
base_url = "https://api.openai.com/v1"
# api_key comes from $OPENAI_API_KEY by default; do not put it here in plaintext.

[providers.anthropic]
# Claude Haiku / Sonnet / Opus via the Messages API.
api_key_env = "ANTHROPIC_API_KEY"
base_url = "https://api.anthropic.com"
tool_calling = "native"
# `models[0]` is the model selected when running `/provider set anthropic`.
# Haiku is the default for the Anthropic provider.
models = ["claude-haiku-4-5", "claude-sonnet-4-6", "claude-opus-4-7"]

[providers.mistral]
# Mistral Chat Completion API.
api_key_env = "MISTRAL_API_KEY"
base_url = "https://api.mistral.ai/v1"
tool_calling = "native"
# `models[0]` is the model selected when running `/provider set mistral`.
models = ["mistral-medium-3-5", "mistral-small-2603", "mistral-large-2512", "devstral-2512", "codestral-2508"]

[policy]
# Per-risk-level default action: "allow" | "confirm" | "confirm_strong" | "deny".
read_only = "allow"
low_risk = "allow"
write = "confirm"
destructive = "confirm_strong"
network = "confirm"
privileged = "confirm_strong"
unknown = "confirm"

# Additional sensitive path patterns (built-in patterns are always included).
sensitive_paths = [
  "~/.ssh/**",
  "**/.env",
  "**/credentials*",
]

# Filesystem allowed roots. Advisory only since v0.2.7 — surfaced to the
# agent in the system prompt, not enforced by the policy engine. $CWD is the
# launch directory.
allowed_roots = [
  "$CWD",
  "$HOME/projects",
]

[tools]
# Per-tool timeouts in milliseconds.
read_file_timeout_ms = 5000
list_directory_timeout_ms = 5000
run_process_timeout_ms = 30000

[audit]
# Override the audit log directory. Default: "~/.llmsh/sessions".
# directory = "/var/log/llmsh"

[agent]
# Bounds for the iterate-until-done loop.
max_iterations = 10
max_tool_calls_per_iteration = 8
max_schema_repair_attempts = 2
```

## Project-level `.llmsh.toml`

Place a `.llmsh.toml` in the project root to override settings for that project. Common uses:

```toml
default_model = "openai:gpt-4o"

[policy]
allowed_roots = ["$CWD"]   # restrict to this project only.
write = "confirm_strong"   # be stricter about writes here.
```

The merge is a per-key shallow merge: project keys override user keys; user keys override defaults.

## Environment variables

| Variable | Effect |
|---|---|
| `OPENAI_API_KEY` | Required for the OpenAI-compatible provider. |
| `ANTHROPIC_API_KEY` | Required for the Anthropic provider (Claude Haiku/Sonnet/Opus). |
| `MISTRAL_API_KEY` | Required for the Mistral provider. |
| `LLMSH_MODEL` | Override `default_model` for the current session. |
| `LLMSH_CONFIG` | Use a non-default user config path. |
| `LLMSH_DEBUG=1` | Enable tracing on stderr. |
| `LLMSH_VERBOSE=1\|2` | Per-turn verbose stats on stderr (tier 1 = headline, tier 2 = detailed). Equivalent to CLI `-v` / `-vv`. |
| `LLMSH_NO_AUDIT=1` | Disable the audit log (not recommended outside tests). |
| `LLMSH_NO_AUTOINIT=1` | Skip the bootstrap `/init` on first launch. |
| `LLMSH_MEMORY_DB` | Override the memory SQLite path. Empty value is treated as unset. |

## CLI flags

| Flag | Effect |
|---|---|
| `-v` | Verbose tier 1: per-turn token usage + tool counts to stderr. Equivalent to `LLMSH_VERBOSE=1`. |
| `-vv` | Verbose tier 2: tier 1 + per-tool timings, policy decisions, redaction hits. Equivalent to `LLMSH_VERBOSE=2`. |
| `--config <path>` | Load configuration from `<path>` instead of the default user config (see "File locations" above). Equivalent to `LLMSH_CONFIG=<path>`. |

## Slash commands

Inside the REPL, slash commands operate on the running session. Source of truth: the `HELP_TEXT` string in `crates/llmsh-core/src/repl.rs`.

**Session**

| Command | Effect |
|---|---|
| `/help` | List available slash commands. |
| `/exit` | Quit (Ctrl-D also works). |

**Context & memory**

| Command | Effect |
|---|---|
| `/clear-context` | Drop the current conversation history (this session). Emits a `context_cleared` audit event. |
| `/clear-memory` | Drop all curated long-term facts. |
| `/clear-all` | Both of the above. |
| `/compact` | Summarize older messages to free context budget. Emits a `context_compacted` audit event. |
| `/memory list` | List curated long-term facts. |
| `/memory forget <id>` | Remove fact `#<id>`. |
| `/memory add [cat:]<claim>` | Add a fact. Categories: `identity`, `preference`, `project`, `todo`, `other` (default). Emits a `fact_added` audit event. |

**Filesystem**

| Command | Effect |
|---|---|
| `/pwd` | Print the current working directory. |
| `/cd <path>` | Change directory. Emits a `cwd_changed` audit event with `source = "meta"`. |

**History**

| Command | Effect |
|---|---|
| `/history` | Print the last 20 inputs of this session. |

**Model**

| Command | Effect |
|---|---|
| `/model` | Show the current model. |
| `/model list` | List models offered by the provider (chat-only filter, 60 s cache). |
| `/model set <provider:model>` | Switch the active model. The change is persisted to `default_model` atomically. |

**Init**

| Command | Effect |
|---|---|
| `/init` | Run the machine audit and persist the result to memory. Auto-bootstrapped on first launch unless `LLMSH_NO_AUTOINIT=1`. |

## Raw shell

Prefix a line with `!` to execute it as raw shell (e.g. `!ls -la`). Raw shell still goes through the policy engine and the audit log. There is no off-the-record execution path.

## Permissions

| Path | Mode |
|---|---|
| User `config.toml` (see "File locations") | `0o600` |
| `~/.llmsh/sessions/` | `0o700` |
| audit files | `0o600` |

These are enforced when LLMShell creates the files. If you create them manually with looser permissions, LLMShell will not silently widen them — it will refuse to write or warn.
