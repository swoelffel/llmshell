---
name: Bug report
about: Report a defect in LLMShell
title: "[bug] "
labels: bug
---

## Description

A clear, concise description of what is wrong.

## Reproduction

Minimal steps to reproduce the behaviour:

1. …
2. …
3. …

If a specific prompt triggers the issue, paste it (redact secrets).

## Expected behaviour

What you expected to happen.

## Actual behaviour

What actually happened. Include relevant audit log lines if helpful (`~/.local/share/llmsh/audit/`).

## Environment

- LLMShell version: `llmsh --version`
- OS / arch:
- Provider + model:
- Rust toolchain (if built from source): `rustc --version`
- Relevant config snippets from the user `config.toml` (`~/.config/llmsh/` on Linux, `~/Library/Application Support/llmsh/` on macOS) and `.llmsh.toml` (redact API keys)

## Audit / policy impact

If the bug touches the policy gate, the confirmation flow, or the audit log, describe what *should* have been recorded vs. what was. Security-sensitive issues should be reported privately — see [SECURITY.md](../../SECURITY.md).

## Additional context

Logs (`LLMSH_DEBUG=1`), screenshots, or anything else useful.
