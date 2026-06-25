# Runbook — Local install / upgrade of `llmsh`

Use this every time you need the freshly built `llmsh` binary on the developer machine (typically after a feature branch, a release bump, or a config-shape change). It replaces ad-hoc `cp` flows which break on macOS Sequoia (provenance xattr → `zsh: killed`).

## Prerequisites

- Workspace builds cleanly: `cargo build --release` produces `target/release/llmsh`.
- `$HOME/.cargo/bin` is on `$PATH`.

## 1. Install the binary

**Preferred — `cargo install`** (signs the binary correctly, no xattr quirks):

```bash
cargo install --path crates/llmsh-cli --force
llmsh --version          # must report the version from Cargo.toml
```

If the harness blocks `cargo install` (sandboxed permission denial), fall back to a manual install. **Always do steps a → b → c together** — a bare `cp` over the previous binary breaks Gatekeeper on macOS Sequoia:

```bash
# a. copy
cp target/release/llmsh "$HOME/.cargo/bin/llmsh"

# b. strip provenance xattr that Sequoia attached to the cp result
xattr -c "$HOME/.cargo/bin/llmsh"

# c. ad-hoc re-sign so dyld will load it
codesign --force --sign - "$HOME/.cargo/bin/llmsh"

# verify
llmsh --version
```

Diagnostic shortcut if a previously-installed `llmsh` suddenly dies with `zsh: killed`:

```bash
xattr -l "$HOME/.cargo/bin/llmsh"     # look for com.apple.provenance
codesign -dv "$HOME/.cargo/bin/llmsh" # expect "Signature=adhoc" (linker-signed is fine)
```

The fix is the same `xattr -c` + `codesign --force --sign -` pair.

## 2. Sync user config with new defaults

`load_or_create_user` reads the existing `config.toml` **as-is** — it does not merge new defaults into an already-present file. Whenever a release adds a provider block, a policy key, or any other top-level section, the user config has to be updated manually.

Config location (per OS via the `directories` crate):

| Platform | Path |
|---|---|
| macOS | `~/Library/Application Support/llmsh/config.toml` |
| Linux | `~/.config/llmsh/config.toml` |
| Windows | `%APPDATA%\llmsh\config.toml` |

Steps:

```bash
# 1. Diff your file against the shipped defaults.
diff <(cargo run -q -p llmsh-cli -- --print-defaults 2>/dev/null \
        || sed -n '/Sample `config.toml`/,/```$/p' docs/configuration.md) \
     "$HOME/Library/Application Support/llmsh/config.toml"
```

`--print-defaults` does not exist yet (tracked as future improvement); until then, the canonical reference is the [Sample `config.toml`](../configuration.md#sample-configtoml) section, kept up-to-date alongside `Config::defaults()` in [crates/llmsh-core/src/config/types.rs](../../crates/llmsh-core/src/config/types.rs).

2. Append (don't replace) the missing blocks. Preserve user overrides — never `mv config.toml{,.bak}` unless explicitly asked.

3. Re-launch `llmsh` and verify with `/provider` / `/model list` that the new provider/model appears.

## 3. Verification gate (Definition of Done)

A deploy is **not** finished until all of these pass on the user shell:

- [ ] `which llmsh` → `~/.cargo/bin/llmsh` (or the user-chosen install path).
- [ ] `llmsh --version` → matches `[workspace.package].version` in `Cargo.toml`. **The new version, not the old one.**
- [ ] For any new provider added in this release: `/provider` lists it.
- [ ] For any new model added: `/model list` returns it (or the configured allowlist contains it).
- [ ] Smoke test: one user turn end-to-end, audit log advances by one event (`tail -n1 ~/.llmsh/sessions/<id>.jsonl`).

If a step fails, **do not** declare the deploy complete. Re-run from §1 or §2 as appropriate; do not silently move on.

## Common failure modes

| Symptom | Cause | Fix |
|---|---|---|
| `zsh: killed llmsh …` (no message) | macOS provenance xattr / broken adhoc sig after `cp` | `xattr -c … && codesign --force --sign - …` |
| `llmsh --version` reports the old version | `cargo install` not run / binary copied to a path not on `$PATH` | re-run §1; verify `which llmsh` |
| `/provider` does not list the new provider | User config predates the release; defaults not merged into existing file | §2 — append the missing block |
| `env var ANTHROPIC_API_KEY not set` | API key not exported in current shell | `export ANTHROPIC_API_KEY=…` in the shell that launches `llmsh` |
| `env var MISTRAL_API_KEY not set` | API key not exported in current shell | `export MISTRAL_API_KEY=…` in the shell that launches `llmsh` |
| `unknown provider "X"; supported: …` | Binary is older than the config; `cargo install` was skipped | re-run §1 |
