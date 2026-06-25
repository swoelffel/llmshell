# Roadmap

LLMShell is pre-1.0 and shipped as experimental software. The roadmap below sketches the next milestones; scope and ordering are subject to change as we get feedback.

## v0.3 — Install & documentation

Goal: drop the time-to-first-session below two minutes.

### Foundation

- `.github/workflows/release.yml` triggered on `v*` tags — builds four targets: `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `x86_64-apple-darwin`, `aarch64-apple-darwin`.
- Each release attaches `llmsh-vX.Y.Z-<target>.tar.gz` archives plus a `SHA256SUMS` file.
- `curl -fsSL https://raw.githubusercontent.com/swoelffel/llmshell/main/install.sh | sh` documented as the official end-user path, with `cargo install --git` retained for source installs.

### Distribution channels

Tiered by reach-per-effort. Foundation above is a prerequisite for every channel except cargo.

- **`install.sh`** (`curl -fsSL https://… | sh`) — universal Linux/macOS path. Detects OS+arch, downloads the matching archive from GitHub Releases, verifies SHA256, installs to `~/.local/bin` by default, then runs `llmsh setup` when a terminal is available.
- **Homebrew tap** (`swoelffel/homebrew-tap`) — `brew install swoelffel/tap/llmshell`. One Ruby formula with `on_macos` + `on_linux` blocks covers macOS *and* Linuxbrew users in one shot. Auto-bumped from `release.yml` via [Justintime50/homebrew-releaser](https://github.com/Justintime50/homebrew-releaser) or a small `sed` step.
- **AUR `llmsh-bin`** — `yay -S llmsh-bin`. A `PKGBUILD` pointing at the GitHub Release archives. Low effort, strong reach in the Arch / Manjaro / dev-tools community.

### Deferred to v0.4+ (only if adoption warrants the maintenance cost)

- `.deb` repo (Debian / Ubuntu / Mint) — built via `cargo-deb`, distributed through Cloudsmith or a self-hosted APT repo with GPG signing.
- `.rpm` repo (Fedora / RHEL / openSUSE) — built via `cargo-generate-rpm`, distributed through Copr or OBS.
- Nix flake — fits the safety-first positioning (deterministic, sandbox-friendly).
- GHCR container image (`ghcr.io/swoelffel/llmsh`) — useful for CI / voluntary sandboxing, less natural for an interactive shell.
- `asdf` / `mise` plugin — small wrapper around the GitHub Releases for polyglot devs.
- crates.io publication of `llmsh-cli` (and possibly the supporting crates).

Snap and Flatpak are **not** on the roadmap: their confinement model interacts badly with `run_process` and arbitrary-path tools, which would push us to "classic" / unconfined modes that defeat the point.

### Documentation & demo

- README demo asset (asciinema cast or short GIF) above the fold.
- `docs/safety.md` finalised and linked from the safety section.
- Release notes oriented for end users, not commit-by-commit.

## v0.4 — Local-first provider

Goal: usable offline / on a laptop without sending data to a hosted provider.

- Ollama provider behind the `LlmProvider` trait.
- Provider diagnostics: `llmsh /doctor` reports reachability, model availability, capability flags.
- Capability detection so the agent loop adapts to providers without parallel tool calls or JSON mode.

## v0.5 — Developer workflows

Goal: useful daily-driver for code-aware tasks.

- `git_status`, `git_diff` typed tools.
- Project-aware context (detect language, build system, test command).
- Slash commands for common workflows (`/test`, `/lint`, `/explain`).

## v0.6 — Controlled write operations

Goal: safe write tools without surprises.

- `write_file`, `edit_file` typed tools.
- Preview / dry-run mode that shows the diff before applying.
- Stronger confirmation UX for write operations.

## v0.7 — Provider compatibility

Goal: work with non-OpenAI providers that don't expose tool-calling natively.

- JSON-fallback provider (model returns structured tool calls in plain JSON).
- Capability-driven selection in the agent loop.
- Tool-call eval suite to compare providers on the same benchmarks.

## Beyond v0.7

These are tracked but unscheduled:

- crates.io publication.
- Sandboxed execution (Linux: bubblewrap / seccomp; macOS: sandbox-exec) — would replace "not a sandbox" caveats in the safety model.
- Multi-session memory across runs.
- Plugin tools loaded at runtime (out-of-tree).

## Phase 5 launch (post-v0.3)

Once the install path is smooth and the demo asset exists, run a controlled launch on Show HN, Lobsters, Reddit, LinkedIn. Until then, the project is intentionally low-profile.
