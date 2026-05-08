---
paths:
  - "crates/llmsh-policy/**/*.rs"
  - "crates/llmsh-core/src/pipeline.rs"
  - "crates/llmsh-core/src/confirm.rs"
  - "crates/llmsh-core/src/raw_shell.rs"
---

# Policy & confirmation rules

Canonical spec: the LLMShell CDC under `ai-docs/`.

## Required behaviour

- `RiskAction` (`Allow` / `Confirm` / `Deny`) is decided by `PolicyEngine`. The model never has authority over execution — its output cannot short-circuit the policy decision.
- A `RiskAction::Confirm` outcome must reach `ConfirmationGate::confirm`. Do not implement an ad-hoc prompt branch outside the gate; tests rely on substituting `AlwaysYesGate` / `AlwaysNoGate` to script behaviour.
- `PolicyContext.sensitive_path_patterns` is checked **before** the tool runs. A read or write that resolves a path post-policy is a regression.
- Bound every loop with `AgentBounds` (`max_iterations`, `max_tool_calls_per_iteration`, `max_schema_repair_attempts`). New code paths must respect these counters.

## Neutral-types boundary

`llmsh-core` never imports `llmsh_llm_openai`. It depends only on the neutral types in `llmsh-llm`. Verify with:

```
rg llmsh_llm_openai crates/llmsh-core/src/
```

A non-empty result outside tests/comments is a regression.

## Adding a new policy decision

When introducing a new `RiskLevel` heuristic in [crates/llmsh-policy/src/engine.rs](crates/llmsh-policy/src/engine.rs):

1. Update `DefaultPolicyConfig` with the per-level action mapping.
2. If the new heuristic involves a path, route it through `paths.rs` so workspace-boundary canonicalisation is applied uniformly.
3. Cover the decision with an e2e test in `crates/llmsh-core/tests/` — see the e2e-test-pattern rule.

## Not a gate

This rule is guidance the model reads when editing policy-adjacent code. The actual gate is `PolicyEngine` itself.
