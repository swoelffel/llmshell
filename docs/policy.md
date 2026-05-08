# Policy engine

The policy engine classifies every tool call before it runs. The model has no authority over the decision — its output cannot short-circuit the policy.

## Risk levels

[`RiskLevel`](../crates/llmsh-policy/src/types.rs) classifies the *kind* of operation:

| Level | Examples |
|---|---|
| `read_only` | `read_file`, `list_directory` against allowed roots. |
| `low_risk` | Cheap, idempotent, side-effect-free commands. |
| `write` | File writes, mutations to project state. |
| `destructive` | `rm`, `git reset --hard`, drop tables. |
| `network` | Anything that reaches a remote endpoint. |
| `privileged` | `sudo`, capability changes. |
| `unknown` | Default for unclassified inputs — treated as the safest reasonable action. |

## Actions

[`PolicyAction`](../crates/llmsh-policy/src/types.rs) is what the engine *does* with a step:

| Action | Meaning |
|---|---|
| `Allow` | Run without prompting. |
| `RequireConfirmation { strong, phrase }` | Surface a prompt with the resolved arguments and policy flags. `strong: true` requires the user to type a specific phrase to confirm. |
| `Deny` | Block the call. The reason is surfaced to the user and recorded in the audit log. |

The user-facing shorthand is **Allow / Confirm / Deny**; `Confirm` covers both regular and strong confirmation.

## Flags

[`PolicyFlag`](../crates/llmsh-policy/src/types.rs) records *why* a step landed on a given action:

- `SensitivePath` — argument resolves to a path matching `PolicyContext.sensitive_path_patterns`.
- `SecretLikeContent` — content resembles credentials.
- `OutsideWorkspace` — path resolves outside the configured allowed roots.
- `LargeBlastRadius` — a pattern suggesting wide impact (e.g. `rm -rf /`).
- `UsesShell` — raw shell or shell-like meta-characters.
- `UsesPrivilegeEscalation` — `sudo`, `doas`, capability changes.

Multiple flags can fire on the same step. They are recorded together with the decision in the `PolicyDecision` audit event.

## How a decision is made

1. The pipeline enriches the tool call with its declared schema and resolves any path arguments.
2. `PolicyEngine` runs:
   - phrase heuristics (`crates/llmsh-policy/src/phrase.rs`),
   - sensitive-path detection (`crates/llmsh-policy/src/sensitive.rs`),
   - workspace-boundary canonicalisation (`crates/llmsh-policy/src/paths.rs`).
3. The result is a `PolicyDecision { effective_risk, action, flags, reasons }`.
4. The `DefaultPolicyConfig` (and the user's overrides) map `effective_risk` to a default `PolicyAction`. Flags can escalate the action (e.g. a `read_only` call to a `SensitivePath` becomes `Deny`).

The decision is emitted to the audit log as a `PolicyDecision` event before the tool runs.

## Configuration

Edit `~/.config/llmsh/config.toml`:

```toml
[policy]
# Per-risk-level default action.
read_only = "allow"
low_risk = "allow"
write = "confirm"
destructive = "confirm_strong"
network = "confirm"
privileged = "deny"
unknown = "confirm"

# Sensitive path patterns (added to the built-in defaults).
sensitive_paths = [
  "~/.ssh/**",
  "**/.env",
  "**/credentials*",
]

# Filesystem allowed roots (everything else is OutsideWorkspace).
allowed_roots = [
  "$CWD",
  "$HOME/projects",
]
```

A project-level `.llmsh.toml` merges on top of the user config.

See [configuration.md](configuration.md) for the full schema.

## Sensitive paths

`PolicyContext.sensitive_path_patterns` is checked **before** the tool runs. A read or write that resolves a path post-policy is a regression.

Built-in patterns target SSH keys, OS keychains, common credential files, and well-known system locations. They are additive — the user's `sensitive_paths` extends, never replaces, the built-in set.

## Confirmation gate

When the action is `RequireConfirmation`, the pipeline calls `ConfirmationGate::confirm` with the resolved arguments and the flag list. The default reedline-backed gate prompts the user. Tests substitute `AlwaysYesGate` / `AlwaysNoGate` to script behaviour.

There is no ad-hoc prompt branch outside the gate — every confirm path goes through the trait. This keeps the policy/audit story consistent.

## Raw shell

A line prefixed with `!` (e.g. `!ls -la`) bypasses tool routing but **not** the policy engine. The line is classified, gated, and audited like any other call. `crates/llmsh-core/src/raw_shell.rs` handles the risk scan; `UsesShell` is added to the flags.

## Adding a new policy decision

1. Update `DefaultPolicyConfig` with the per-level action mapping.
2. If the new heuristic involves a path, route it through `paths.rs` so workspace-boundary canonicalisation is applied uniformly.
3. Cover the decision with an e2e test in `crates/llmsh-core/tests/` — see [`.claude/rules/e2e-test-pattern.md`](../.claude/rules/e2e-test-pattern.md).

## Neutral-types boundary

`llmsh-core` never imports `llmsh_llm_openai`. It depends only on the neutral types in `llmsh-llm`. Verify with:

```bash
rg llmsh_llm_openai crates/llmsh-core/src/
```

A non-empty result outside tests/comments is a regression.

## References

- Source: [`crates/llmsh-policy/`](../crates/llmsh-policy/).
- Invariants: [`.claude/rules/policy-rules.md`](../.claude/rules/policy-rules.md).
- Canonical brief: [`ai-docs/briefs/archived/LLMShell_CDC_MVP.md`](../ai-docs/briefs/archived/LLMShell_CDC_MVP.md).
