# llmsh-core

Agent loop, REPL, configuration, and pipeline orchestration for LLMShell. Contains the iterative `Agent` (LLM call → tool-call fan-out → confirmation gate → execution → next iteration), the `Repl` that wraps `reedline` for interactive input and slash commands, the TOML `Config` loader with user/project merge, the `Pipeline` that enriches tool schemas and enforces policy, and the `ToolExecutor` that runs tools with timeout and cancellation. This crate is the integration hub: it wires together `llmsh-llm`, `llmsh-tools`, `llmsh-policy`, and `llmsh-audit` into a coherent, cancellable agent session.

## `run_process` policy enrichment

The pipeline's policy step has two enrichment passes specific to
`run_process`:

1. A deterministic classifier (`llmsh_policy::safe_commands`) downgrades
   safe invocations from `Unknown` → `ReadOnly`.
2. The LLM's own `claimed_risk` field is taken into account, but only as
   an **upgrade** — it can never reduce the engine's verdict.

Toggle the classifier via `policy.run_process.auto_classify_read_only`
in the user config (default `true`).
