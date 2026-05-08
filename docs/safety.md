# Safety model

LLMShell is built around a single principle: **the LLM proposes, the runtime decides.** This page explains what that means in practice — what the agent can and cannot do, where the gates are, and what the system is *not*.

## What the LLM can do

- Read messages from the user.
- Plan a sequence of actions.
- Emit tool calls with structured arguments matching a tool schema.
- Reply in natural language to the user.

## What the LLM cannot do

- Execute a tool directly. Every call goes through the pipeline.
- Bypass policy classification.
- Bypass the confirmation gate for `Confirm`-level actions.
- Reach a sensitive path that the policy denies.
- Write to the audit log. The runtime writes events; the LLM cannot author or mutate them.
- Disable redaction.

## Typed tools vs raw shell

LLMShell prefers **typed tools** over raw shell:

- `read_file(path)` — read a file inside an allowed root.
- `list_directory(path)` — list directory contents.
- `run_process(program, args, …)` — execute a subprocess.

Typed tools are introspectable: the policy engine sees structured arguments instead of a free-form command string. Raw shell is still available — prefix a line with `!` — but it is also classified, gated, and audited. There is no "off the record" execution path.

## Policy decisions

Every tool call (typed or raw) is classified by `PolicyEngine` into a `RiskAction`:

| Action | Meaning |
|---|---|
| `Allow` | Run the tool without prompting. |
| `Confirm` | Prompt the user with the resolved arguments + policy flags. Run only on explicit yes. |
| `Deny` | Block the tool call. Surface the reason to the user and to the audit log. |

The classification uses phrase heuristics, the tool name, the resolved path (for filesystem tools), and configuration overrides from `~/.config/llmsh/config.toml`. Details: [policy.md](policy.md).

The model has no authority over the decision — its output cannot short-circuit the policy. Confirm-level actions must reach `ConfirmationGate::confirm`; there is no ad-hoc prompt branch outside the gate.

## Sensitive paths

`PolicyContext.sensitive_path_patterns` is checked **before** the tool runs. A read or write that resolves a path post-policy is a regression.

Default sensitive patterns include SSH keys, credential files, OS keychain locations, and well-known system directories. The exact list is owned by the policy engine and configurable.

## Confirmation prompts

`Confirm`-level actions surface:

- the tool name,
- the resolved arguments (after path canonicalisation),
- the policy flags that triggered the prompt.

Tests use `AlwaysYesGate` / `AlwaysNoGate` to script the behaviour. Production prompts go through reedline.

## Redacted audit logs

Every decision and result lands in an append-only JSONL audit log:

- One line per `AuditEvent`, with a hash-chained `digest` linking each line to the previous.
- All text fields go through `Redactor::default_audit()` before the write.
- Tool outputs returned to the LLM also pass through a separate `llm_redact` step so sensitive content does not re-enter the conversation.
- Audit directory permissions: `0o700`. Audit files: `0o600`. Config: `0o600`.

Details: [audit.md](audit.md).

## Bounded agent loop

The agent loop is bounded by `AgentBounds`:

- `max_iterations` — total turns per user message.
- `max_tool_calls_per_iteration` — protects against runaway tool fan-out.
- `max_schema_repair_attempts` — caps automatic recovery from malformed tool calls.

A misbehaving model cannot keep the loop running indefinitely.

## Known limitations

LLMShell is **not a sandbox**.

- Allowed tool calls run with the full privileges of the user invoking `llmsh`. There is no OS-level isolation.
- `run_process` can spawn arbitrary subprocesses (subject to the policy decision).
- Raw shell mode (`!`) inherits the same privileges.
- The redactor is best-effort. Configure providers you trust, and review your config before enabling auto-confirm for risky tools.
- LLMShell is **experimental software**. Do not run it on production systems or sensitive environments without first reviewing your policy configuration and allowed roots.

OS-level sandboxing (bubblewrap / seccomp on Linux, sandbox-exec on macOS) is on the [roadmap](../ROADMAP.md) but not yet implemented.

## Threat model

LLMShell defends against:

- **Confused-deputy tool calls** — the model is tricked into running something the user did not intend. Mitigated by typed tools + policy + confirmation.
- **Sensitive-path exfiltration** — the model attempts to read SSH keys or credentials. Mitigated by sensitive-path denial.
- **Audit tampering** — a malicious or buggy code path tries to skip an audit write. Mitigated by the hash chain and the requirement that *every* execution path emits an event on success, error, and cancellation.
- **Secret re-injection** — secrets in tool output flow back to the model. Mitigated by `llm_redact` at the LLM boundary.

LLMShell does **not** defend against:

- A user who configures the policy to auto-confirm everything.
- A compromised LLM provider that reads or stores prompts and tool output.
- OS-level escalation from a tool call the policy explicitly allowed.
- Side-channel attacks on the host machine.

If you find a way to bypass the policy, the audit chain, or the redactor, that is a security issue — see [SECURITY.md](../SECURITY.md).

## References

- Canonical brief: [LLMShell_CDC_MVP.md](../ai-docs/briefs/archived/LLMShell_CDC_MVP.md) — risk levels, redaction rules, confirmation semantics.
- Policy invariants: [.claude/rules/policy-rules.md](../.claude/rules/policy-rules.md).
- Audit invariants: [.claude/rules/audit-invariants.md](../.claude/rules/audit-invariants.md).
