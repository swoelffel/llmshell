---
name: Feature request
about: Suggest a new capability or behaviour change
title: "[feature] "
labels: enhancement
---

## Problem

What user-facing problem does this feature address? Who hits it, and how often?

## Proposed solution

Describe the change you would like to see. Be concrete:

- which crate(s) it touches,
- which user-visible surface changes (slash command, config field, prompt, audit event…),
- new tools, if any.

## Alternatives considered

Other approaches you weighed and why this one is preferred.

## Risk / safety impact

LLMShell is safety-first. New capabilities almost always interact with the policy engine, the confirmation gate, or the audit log. Please describe:

- new `RiskLevel` or `PolicyFlag` involved (if any),
- whether the feature can run unattended (`Allow`) or must require confirmation,
- new audit events emitted on success / error / cancel paths,
- any sensitive data that needs redaction.

If unsure, leave a note — reviewers will flag it.

## Additional context

Links to related issues, prior art in other projects, or design sketches.
