# Examples

Illustrative sessions showing how LLMShell behaves in common situations. Output is stylised — exact wording depends on the model.

## Exploring a repository

```text
$ llmsh
llmsh> what does this project do?
[tool] list_directory(path=".")
[tool] read_file(path="README.md")
[assistant] LLMShell is a safety-first agentic shell. The README highlights
typed tools, policy gating, sensitive-path detection, and a redacted audit
log. There are seven Rust crates under crates/, with llmsh-cli producing
the `llmsh` binary.

llmsh> show me the audit event taxonomy
[tool] read_file(path="crates/llmsh-audit/src/event.rs")
[assistant] AuditEvent has 15 variants: SessionStarted, UserInput,
LlmRequest, LlmResponse, ModelPlan, PolicyDecision, ConfirmationAsked,
ToolExecutionStart, ToolExecutionEnd, RawShellExecution, AssistantMessage,
Error, SessionEnded, MachineAuditPerformed, ModelChanged.
```

Both calls were `read_only` against allowed roots — they ran without prompting.

## Sensitive path is denied

```text
llmsh> read ~/.ssh/id_rsa
[policy] denied: sensitive path
  tool:    read_file
  path:    /Users/me/.ssh/id_rsa
  flags:   [SensitivePath]
  reason:  matches built-in sensitive_paths pattern "~/.ssh/**"
[assistant] I cannot read that file — it matches a sensitive path pattern.
```

The policy decision is recorded in the audit log as a `policy_decision` event with `effective_risk = read_only`, `action = deny`, and `flags = ["sensitive_path"]`. No `tool_execution_start` event is emitted because the tool never ran.

## Confirmation prompt before a write

```text
llmsh> rename README.md to README.old.md
[tool] run_process(program="mv", args=["README.md", "README.old.md"])
[policy] confirm: write
  flags:   [UsesShell]
  reason:  default action for risk_level=write
Run this command? [y/N]: y
[tool] exit_code=0
[assistant] Done. README.md is now README.old.md.
```

Audit trail (relevant events):

```jsonl
{"type":"policy_decision","effective_risk":"write","action":{"kind":"require_confirmation","strong":false}, "flags":["uses_shell"], "reasons":["default action for risk_level=write"], …}
{"type":"confirmation_asked","granted":true, …}
{"type":"tool_execution_start","tool":"run_process","args_digest":"sha256:…", …}
{"type":"tool_execution_end","status":"ok","exit_code":0, …}
```

If you answer `n`, a `confirmation_asked` event is recorded with `granted = false` and the tool is **not** executed.

## Strong confirmation for a destructive call

```text
llmsh> drop the dist directory
[tool] run_process(program="rm", args=["-rf", "dist"])
[policy] confirm (strong): destructive
  flags:   [LargeBlastRadius, UsesShell]
  reason:  rm -rf pattern; default action for risk_level=destructive
To confirm, type:  delete dist
> delete dist
[tool] exit_code=0
```

Strong confirmation requires typing the suggested phrase verbatim. Anything else cancels.

## Raw shell escape

```text
llmsh> !grep -r TODO crates/ | head -5
[raw shell] policy: allow (read_only)
crates/llmsh-core/src/agent.rs:42:    // TODO: bound retries
…
```

The `!` prefix routes the line through the raw-shell handler. It is still classified by the policy engine and audited as a `raw_shell_execution` event. Raw shell does not bypass sensitive-path checks:

```text
llmsh> !cat ~/.ssh/id_rsa
[raw shell] policy: deny (sensitive_path)
```

## Switching models mid-session

```text
llmsh> /model
openai:gpt-4o-mini

llmsh> /model list
openai:gpt-4o
openai:gpt-4o-mini
openai:o1-mini
…

llmsh> /model set openai:gpt-4o
[ok] active model: openai:gpt-4o
[ok] persisted as default_model in ~/.config/llmsh/config.toml
```

A `model_changed` audit event is emitted. The `default_model` field is written atomically while preserving the rest of the file and the existing `0o600` permissions.

## Bootstrap on first launch

```text
$ llmsh
[init] no machine audit found — running /init
[init] cwd:    /Users/me/projects/demo
[init] os:     darwin 25.3.0
[init] disk:   42.1 GB free of 500 GB
[init] persisted machine audit to memory.
llmsh>
```

A `machine_audit_performed` event is recorded. Subsequent launches reuse the stored audit unless you re-run `/init` manually.

## Where to look in the audit log

```bash
# Most recent session, filtered to policy decisions and confirmations.
ls -t ~/.local/share/llmsh/audit/ | head -1 \
  | xargs -I{} jq -c 'select(.type == "policy_decision" or .type == "confirmation_asked")' \
      ~/.local/share/llmsh/audit/{}
```

Every example above produces a chain of events you can inspect, filter, or replay.
