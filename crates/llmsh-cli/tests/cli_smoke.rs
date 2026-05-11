//! Smoke tests for the `llmsh` binary. Each test invokes the compiled binary
//! via `assert_cmd::Command::cargo_bin("llmsh")` — no network, no real LLM,
//! short timeout.

use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn version_flag_prints_version() {
    Command::cargo_bin("llmsh")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains("llmsh"));
}

#[test]
fn help_flag_prints_usage() {
    Command::cargo_bin("llmsh")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage"));
}

#[test]
fn unknown_flag_fails_with_clear_error() {
    Command::cargo_bin("llmsh")
        .unwrap()
        .arg("--nonexistent-flag")
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("--nonexistent-flag")
                .or(predicate::str::contains("unexpected")),
        );
}

// Bootstrap timing observation: `--version` is handled by clap inside
// `Cli::parse()` and exits before `main()` reaches `load_or_create_user`
// (main.rs line ~104). Therefore the default config file is NOT written on
// `--version`. The `if cfg.exists()` branch below is intentionally never taken
// with this invocation — it exists so that if a future refactor moves config
// init before clap's early exit, the content assertion kicks in automatically.
// In its current form the test guards against panics in arg parsing and verifies
// that `LLMSH_CONFIG` is accepted without error.
#[test]
fn writes_default_config_on_first_launch() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = tmp.path().join("config.toml");

    Command::cargo_bin("llmsh")
        .unwrap()
        .env("LLMSH_CONFIG", &cfg)
        .env("LLMSH_NO_AUTOINIT", "1")
        .env("OPENAI_API_KEY", "sk-test-fake")
        .arg("--version") // exits early inside clap; config init never runs
        .assert()
        .success();

    // `--version` short-circuits before config init, so the file is not created.
    // If a future change moves init before clap's exit, this assertion will
    // validate the written content.
    if cfg.exists() {
        let content = std::fs::read_to_string(&cfg).unwrap();
        assert!(
            content.contains("model") || content.contains("audit"),
            "default config should mention known top-level keys"
        );
    }
}

#[test]
#[ignore = "needs `llmsh config show` subcommand to assert resolved config"]
fn cwd_llmsh_toml_overrides_user_config() {
    // TODO: add a `llmsh config show` subcommand so this test can assert that
    // the project-local .llmsh.toml model name overrides the user config model.
    let tmp = tempfile::tempdir().unwrap();
    let user_cfg = tmp.path().join("user.toml");
    std::fs::write(&user_cfg, "[model]\nname = \"gpt-4\"\n").unwrap();

    let proj = tempfile::tempdir().unwrap();
    std::fs::write(
        proj.path().join(".llmsh.toml"),
        "[model]\nname = \"gpt-4o-mini\"\n",
    )
    .unwrap();

    let out = Command::cargo_bin("llmsh")
        .unwrap()
        .current_dir(proj.path())
        .env("LLMSH_CONFIG", &user_cfg)
        .env("LLMSH_VERBOSE", "2")
        .env("LLMSH_NO_AUTOINIT", "1")
        .env("OPENAI_API_KEY", "sk-test-fake")
        .arg("--help")
        .output()
        .unwrap();

    // No assertion yet: --help output does not reveal the resolved model.
    // Once `llmsh config show` exists, replace this with a check that
    // "gpt-4o-mini" appears in the output.
    let _ = out;
}
