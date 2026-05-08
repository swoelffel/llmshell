## Summary

What does this PR change, and why? Reference any linked issue (`Closes #123`).

## Test plan

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings`
- [ ] `cargo test --workspace --locked`
- [ ] New behaviour has a test (unit or e2e under `crates/llmsh-core/tests/`).
- [ ] Manually exercised the feature in `llmsh` (when applicable).

## Audit / policy impact

LLMShell is a security-sensitive codebase. Describe any impact on:

- the policy engine (new `RiskLevel`, `PolicyFlag`, or default action mapping),
- the confirmation gate (new prompt branches, new strong-confirmation phrases),
- the audit log (new `AuditEvent` variants, schema bump),
- the redactor (new fields that may carry secrets).

Write "none" if the PR truly does not touch these surfaces.

## CHANGELOG

- [ ] Added an entry under `[Unreleased]` in `CHANGELOG.md` (or this PR is doc-only).

## Notes for reviewers

Anything specific you would like the reviewer to focus on, known gaps, or follow-up issues.
