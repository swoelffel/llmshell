# Contributing

This page is the public quick reference for contributing to LLMShell. It covers the expected Git flow, local verification, and where to find the deeper contributor documentation.

For full development setup, architecture notes, and security expectations, see the repository-level [CONTRIBUTING.md](../CONTRIBUTING.md).

## Recommended Git flow

1. Fork the repository or create a branch from `main`.
2. Create a focused branch for one change.
3. Make the change with tests or docs updated in the same branch.
4. Run the local verification gates before opening a pull request.
5. Open a pull request against `main` with a short summary, test evidence, and any policy or audit impact called out explicitly.

Example:

```bash
git clone https://github.com/swoelffel/llmshell
cd llmshell
git switch -c docs/onboarding-update
```

## Local verification

Match CI before you ask for review:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked
```

For documentation-only changes, run the relevant docs checks as well, for example `git diff --check` on the files you touched.

## Installing a local build

If your change requires testing a rebuilt binary on your machine, use the developer runbook in [docs/runbooks/local-install.md](runbooks/local-install.md). For normal end-user installation, use the release installer shown in the [README](../README.md).

## Pull request expectations

- Keep each pull request narrow enough to review in one pass.
- Include user-facing documentation with any onboarding, configuration, or command changes.
- Call out policy, audit, and secret-handling impact for any runtime or safety-sensitive change.
- Link the issue or task brief when the change is part of a tracked stream of work.

## Security

Please do not open a public issue for vulnerabilities. Follow [SECURITY.md](../SECURITY.md).
