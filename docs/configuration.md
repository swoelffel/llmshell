# Configuration

LLMShell merges three layers, each overriding the previous:

1. Built-in defaults.
2. User config: `~/.config/llmsh/config.toml`.
3. Project config: `.llmsh.toml` in the current working directory (optional).

The first launch writes a default user config; missing files at lower layers are silently ignored.

## File locations

| Path | Purpose |
|---|---|
| `~/.config/llmsh/config.toml` | User config (mode `0o600`). |
| `~/.config/llmsh/AGENTS.md` | User-level agent instructions, loaded into the system prompt with a 2 KiB budget. |
| `.llmsh.toml` (cwd) | Project overrides. Merged on top of the user config. |
| `~/.local/share/llmsh/audit/` | Audit log directory (mode `0o700`). |
| `~/.local/share/llmsh/memory.sqlite` | Long-term memory store. |

Override the config path with `--config <path>` or `LLMSH_CONFIG`.

## Sample `config.toml`

```toml
# Default model. Format: "<provider>:<model-name>".
default_model = "openai:gpt-4o-mini"

[providers.openai]
# Optional. Defaults to OpenAI's hosted endpoint.
base_url = "https://api.openai.com/v1"
# api_key comes from $OPENAI_API_KEY by default; do not put it here in plaintext.

[policy]
# Per-risk-level default action: "allow" | "confirm" | "confirm_strong" | "deny".
read_only = "allow"
low_risk = "allow"
write = "confirm"
destructive = "confirm_strong"
network = "confirm"
privileged = "deny"
unknown = "confirm"

# Additional sensitive path patterns (built-in patterns are always included).
sensitive_paths = [
  "~/.ssh/**",
  "**/.env",
  "**/credentials*",
]

# Filesystem allowed roots. $CWD is the launch directory.
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
# Override the audit log directory.
# dir = "/var/log/llmsh"

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
| `LLMSH_MODEL` | Override `default_model` for the current session. |
| `LLMSH_CONFIG` | Use a non-default user config path. |
| `LLMSH_DEBUG=1` | Enable tracing on stderr. |
| `LLMSH_NO_AUDIT=1` | Disable the audit log (not recommended outside tests). |
| `LLMSH_MEMORY_DB` | Override the memory SQLite path. Empty value is treated as unset. |

## Slash commands

Inside the REPL, slash commands operate on the running session:

| Command | Effect |
|---|---|
| `/help` | List available slash commands. |
| `/model` | Show the current model. |
| `/model list` | List models offered by the provider (chat-only filter, 60 s cache). |
| `/model set <provider:model>` | Switch the active model. The change is persisted to `default_model` atomically. |
| `/init` | Run the machine audit and persist the result to memory. Auto-bootstrapped on first launch. |

## Raw shell

Prefix a line with `!` to execute it as raw shell (e.g. `!ls -la`). Raw shell still goes through the policy engine and the audit log. There is no off-the-record execution path.

## Permissions

| Path | Mode |
|---|---|
| `~/.config/llmsh/config.toml` | `0o600` |
| `~/.local/share/llmsh/audit/` | `0o700` |
| audit files | `0o600` |

These are enforced when LLMShell creates the files. If you create them manually with looser permissions, LLMShell will not silently widen them — it will refuse to write or warn.
