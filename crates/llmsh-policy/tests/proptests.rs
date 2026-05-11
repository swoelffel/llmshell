//! Property-based tests for the LLMShell policy classifier.
//!
//! Each property captures an invariant that no random argv should violate.
//! When proptest finds a counterexample, the shrunk input is persisted under
//! `crates/llmsh-policy/proptest-regressions/` and re-played on every run.
//!
//! # API adaptation note
//!
//! The plan assumed `PolicyEngine::classify(argv, ctx) -> RiskAction`.
//! The real API is different: `DefaultPolicyEngine::evaluate(CheckedToolCall, PolicyContext)
//! -> PolicyDecision`, and the `run_process` argv classification that maps
//! args to `RiskLevel` lives in `llmsh_policy::safe_commands::is_read_only_invocation`.
//!
//! The metachar invariant is properly tested at that layer:
//! "an argv that contains a raw shell metachar in any arg position must not
//! cause `is_read_only_invocation` to return `Some(RiskLevel::ReadOnly)`."
//!
//! This is the correct place — the pipeline calls `is_read_only_invocation`
//! to determine whether to assign `RiskLevel::ReadOnly` (→ `Allow`).
//! Bypassing it here would silently grant `Allow` to a metachar-bearing argv.

// proptest macros generate some code that may trigger pedantic lints.
#![allow(clippy::arc_with_non_send_sync)]

use llmsh_policy::safe_commands::is_read_only_invocation;
use llmsh_policy::RiskLevel;
use proptest::prelude::*;

// === Generators ===

fn shell_metachar() -> impl Strategy<Value = char> {
    prop::sample::select(vec!['|', '&', ';', '>', '<', '`', '$', '(', ')', '\n'])
}

fn safe_token() -> impl Strategy<Value = String> {
    "[a-zA-Z0-9_./-]{1,12}".prop_filter("must not contain metachar", |s| {
        !s.chars().any(|c| {
            matches!(
                c,
                '|' | '&' | ';' | '>' | '<' | '`' | '$' | '(' | ')' | '\n'
            )
        })
    })
}

fn argv_with_metachar() -> impl Strategy<Value = (String, Vec<String>)> {
    (
        prop::sample::select(vec!["ls", "cat", "grep", "echo", "find"]).prop_map(String::from),
        prop::collection::vec(safe_token(), 0..4),
        shell_metachar(),
    )
        .prop_map(|(cmd, mut args, meta)| {
            args.push(format!("inject{meta}payload"));
            (cmd, args)
        })
}

proptest! {
    /// Classification must be deterministic: same argv → same result.
    /// Non-determinism would make audit logs unreliable.
    #[test]
    fn classification_is_deterministic(
        argv in prop::collection::vec(safe_token(), 1..6)
    ) {
        prop_assume!(!argv.is_empty());
        let program = &argv[0];
        let args = &argv[1..];
        let a1 = is_read_only_invocation(program, args);
        let a2 = is_read_only_invocation(program, args);
        prop_assert_eq!(a1, a2);
    }
}

proptest! {
    /// If `cmd` (no args) is classified ReadOnly, adding `--help` (a recognised
    /// help flag) must keep it ReadOnly. Help flags never widen risk.
    /// Catches regressions where a new arg pattern accidentally blocks the
    /// simplest invocations.
    #[test]
    fn help_flag_preserves_read_only_for_safe_commands(
        cmd in prop::sample::select(vec!["ls", "pwd", "whoami", "uname", "date"])
    ) {
        let bare = is_read_only_invocation(cmd, &[]);
        let with_help = is_read_only_invocation(cmd, &["--help".to_string()]);
        if bare == Some(RiskLevel::ReadOnly) {
            prop_assert_eq!(
                with_help,
                Some(RiskLevel::ReadOnly),
                "{} bare was ReadOnly but {} --help was {:?}",
                cmd, cmd, with_help
            );
        }
    }
}

proptest! {
    /// A handful of programs are universally destructive enough that *any*
    /// argv starting with them must NOT be classified as ReadOnly. Tightens
    /// against future regressions that might add an over-eager "safe
    /// subcommand" rule.
    #[test]
    fn destructive_programs_never_read_only(
        prog in prop::sample::select(vec!["rm", "dd", "mkfs", "shred", "fdisk"]),
        rest in prop::collection::vec(safe_token(), 0..5)
    ) {
        let result = is_read_only_invocation(prog, &rest);
        prop_assert!(
            result != Some(RiskLevel::ReadOnly),
            "program={:?} args={:?} was classified ReadOnly", prog, rest
        );
    }
}

proptest! {
    /// An argv that contains a raw shell metachar inside an argument must NOT
    /// be classified as `ReadOnly` by `is_read_only_invocation`. Either the
    /// function rejects the arg (returns `None` → falls through to `Unknown`
    /// → `Confirm`) or it has a structural reason not to match. A bare
    /// `ls foo|bar` is never safe to run silently.
    ///
    /// THIS IS A REAL-BUG-HUNTING TEST. If proptest finds a counterexample,
    /// that is a genuine policy bypass — do NOT loosen the property.
    #[test]
    fn metachar_in_arg_is_never_read_only(
        (program, args) in argv_with_metachar()
    ) {
        let result = is_read_only_invocation(&program, &args);
        prop_assert!(
            result != Some(RiskLevel::ReadOnly),
            "program={:?} args={:?} was classified ReadOnly despite shell metachar",
            program,
            args
        );
    }
}
