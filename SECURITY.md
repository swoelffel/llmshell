# Security Policy

## Reporting a vulnerability

If you believe you have found a security issue in LLMShell, please **do not open a public GitHub issue**.

Email `swoelffel@castelis.com` with:

- a description of the issue,
- a minimal repro (LLMShell version, OS, provider, config snippet if relevant),
- the impact you observed.

You should expect an acknowledgement within 7 days. Coordinated disclosure is preferred — we will agree on a public-disclosure date once a fix is ready.

## Supported versions

LLMShell is pre-1.0. Only the `main` branch and the latest tagged release receive security fixes.

| Version | Supported |
|---|---|
| `main` | ✅ |
| latest release tag | ✅ |
| any earlier tag | ❌ |

## Security model

LLMShell is designed around the principle that **the LLM proposes, the runtime decides**:

- The `ToolRegistry` is the only source of executable tools.
- Every tool call is classified by the policy engine into `Allow` / `Confirm` / `Deny`.
- Sensitive paths (SSH keys, credential files, system directories) are denied by default.
- Risky actions surface a confirmation prompt with the resolved arguments and policy flags before execution.
- Raw shell access requires the explicit `!` prefix and is still classified, gated, and audited.
- All decisions and outcomes are written to a hash-chained, redacted, append-only JSONL audit log.

Full details: [docs/safety.md](docs/safety.md).

## Known limitations

LLMShell is **not a sandbox**. It does not provide OS-level isolation: tool calls that the policy allows run with the privileges of the user running `llmsh`. In particular:

- `run_process` executes real subprocesses with your environment and credentials.
- Raw shell mode (`!`) inherits the same privileges.
- The redactor operates on the LLM boundary; ensure you trust the providers you configure.
- LLMShell is **experimental software**. Do not run it on production systems or sensitive environments without first reviewing your policy configuration and allowed roots.

If you find a way to bypass the policy gate, the audit chain, or the redactor, that is a security issue — please report it.
