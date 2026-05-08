# llmsh-policy

Risk-classification and enforcement engine for LLMShell. Assigns every proposed tool invocation a `RiskLevel` (ReadOnly, LowRisk, Write, Destructive, Network, Privileged, or Unknown) using path analysis, phrase matching, and sensitive-file detection, then maps each level to a configurable `RiskAction` (Allow, Confirm, ConfirmStrong, or Deny). The `PolicyEngine` trait keeps the enforcement logic decoupled from any specific UI, and the `PolicyContext` carries per-request workspace information such as allowed filesystem roots and sensitive path patterns.
