Task 4 report: Release Workflow

Scope completed:
- Added `.github/workflows/release.yml` in the onboarding-install worktree.
- Kept the change scoped to the requested workflow file only.

Implementation summary:
- Added a tag-triggered `release` workflow for tags matching `v*`.
- Configured a four-target build matrix:
  - `x86_64-unknown-linux-gnu` on `ubuntu-latest`
  - `aarch64-unknown-linux-gnu` on `ubuntu-24.04-arm`
  - `x86_64-apple-darwin` on `macos-13`
  - `aarch64-apple-darwin` on `macos-latest`
- Each build job:
  - checks out the repo
  - installs the Rust toolchain for the target
  - restores cargo cache
  - runs `cargo test --workspace --locked`
  - builds `llmsh-cli` in release mode for the target
  - packages `llmsh`, `README.md`, and `LICENSE` into the required tarball name
  - uploads the tarball as a workflow artifact
- Added a `publish` job that downloads all tarballs, generates `SHA256SUMS`, and publishes assets with `softprops/action-gh-release@v2`.

Validation run:
- `git diff --check -- .github/workflows/release.yml`
- Result: passed with no whitespace errors.

Commit:
- `ci: publish release binaries`

Notes / concerns:
- Per the task brief, `ubuntu-24.04-arm` is used directly for the Linux ARM build.
- Repository-side availability of that GitHub-hosted runner cannot be proven locally; if unavailable in CI, the expected follow-up is to switch that leg to cross compilation with `cross`.
