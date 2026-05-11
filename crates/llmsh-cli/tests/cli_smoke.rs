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
