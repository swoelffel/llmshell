//! Deterministic classifier for `run_process` invocations. Returns
//! `Some(RiskLevel::ReadOnly)` only when the (program, args) pair is
//! provably read-only. Anything ambiguous returns `None` so the caller
//! falls back to the existing `RiskLevel::Unknown` → `Confirm` flow.

use crate::types::RiskLevel;

const SHELLS: &[&str] = &[
    "bash", "sh", "zsh", "fish", "dash", "ksh", "csh", "tcsh", "ash",
];

const META_EXEC: &[&str] = &[
    "xargs", "parallel", "env", "nohup", "time", "timeout", "sudo", "doas",
];

const ALWAYS_SAFE: &[&str] = &[
    // FS metadata / listing
    "ls",
    "stat",
    "file",
    "tree",
    "du",
    "df",
    "pwd",
    "realpath",
    "readlink",
    "basename",
    "dirname",
    // Pure read of stdout
    "cat",
    "head",
    "tail",
    "wc",
    "od",
    "hexdump",
    "xxd",
    // Identity & system info
    "whoami",
    "id",
    "groups",
    "uname",
    "hostname",
    "uptime",
    "date",
    "tty",
    "which",
    "type",
    "command",
    "where",
    // Process inspection (no kill/-signal)
    "ps",
    "pgrep",
    "lsof",
    // Pure text processing — programs without an in-place mode
    "sort",
    "uniq",
    "cut",
    "tr",
    "column",
    "paste",
    "fold",
    "fmt",
    "rev",
    "nl",
    "diff",
    "cmp",
    "comm",
    // Search
    "grep",
    "rg",
    "ag",
    "ack",
    // Echo / format
    "echo",
    "printf",
    "yes",
    // Env inspection
    "printenv",
    // Networking probes (no side effects)
    "ping",
    "host",
    "nslookup",
    "dig",
    "traceroute",
    "whois",
    // misc
    "true",
    "false",
    "sleep",
    "tput",
    "clear",
];

const RUNTIMES_VERSION_ONLY: &[&str] = &[
    "python", "python3", "ruby", "node", "deno", "bun", "java", "go", "perl", "php", "lua",
    "rustc", "gcc", "g++", "clang", "clang++",
];

const PKG_MANAGERS: &[&str] = &[
    "npm", "pnpm", "yarn", "cargo", "pip", "pip3", "brew", "apt", "dpkg", "rpm",
];

const PKG_QUERY_SUBCOMMANDS: &[&str] = &[
    "list", "ls", "view", "info", "show", "search", "tree", "metadata", "outdated", "audit",
    "doctor", "config",
];

const GIT_READ_SUBCOMMANDS: &[&str] = &[
    "status",
    "log",
    "diff",
    "show",
    "rev-parse",
    "ls-files",
    "ls-tree",
    "cat-file",
    "blame",
    "describe",
    "reflog",
    "shortlog",
    "remote",
    "branch",
    "tag",
    "stash",
    "for-each-ref",
    "name-rev",
    "merge-base",
    "verify-commit",
    "verify-tag",
    "fsck",
    "check-ignore",
    "check-attr",
    "check-mailmap",
    "ls-remote",
    "fetch-pack",
    "config",
];

pub fn is_read_only_invocation(program: &str, args: &[String]) -> Option<RiskLevel> {
    if program.contains('/') {
        return None;
    }
    if SHELLS.contains(&program) || META_EXEC.contains(&program) {
        return None;
    }

    if ALWAYS_SAFE.contains(&program) {
        return Some(RiskLevel::ReadOnly);
    }

    let safe = match program {
        "find" => find_args_are_read_only(args),
        "sed" => sed_args_are_read_only(args),
        "awk" | "gawk" | "mawk" | "nawk" => false,
        "jq" | "yq" => !args.iter().any(|a| a == "-i" || a == "--in-place"),
        "crontab" => args.len() == 1 && args[0] == "-l",
        "git" => git_args_are_read_only(args),
        p if RUNTIMES_VERSION_ONLY.contains(&p) => is_version_only(args),
        p if PKG_MANAGERS.contains(&p) => pkg_args_are_read_only(args),
        _ => false,
    };
    if safe {
        Some(RiskLevel::ReadOnly)
    } else {
        None
    }
}

pub fn parse_claimed_risk(s: &str) -> Option<RiskLevel> {
    match s {
        "read_only" => Some(RiskLevel::ReadOnly),
        "low" | "low_risk" => Some(RiskLevel::LowRisk),
        "write" => Some(RiskLevel::Write),
        "destructive" => Some(RiskLevel::Destructive),
        "network" => Some(RiskLevel::Network),
        "privileged" => Some(RiskLevel::Privileged),
        "unknown" => Some(RiskLevel::Unknown),
        _ => None,
    }
}

fn is_version_only(args: &[String]) -> bool {
    args.len() == 1
        && (args[0] == "--version" || args[0] == "-V" || args[0] == "-v" || args[0] == "version")
}

fn pkg_args_are_read_only(args: &[String]) -> bool {
    if args.is_empty() {
        return false;
    }
    if is_version_only(args) {
        return true;
    }
    let first = args[0].as_str();
    PKG_QUERY_SUBCOMMANDS.contains(&first)
        && !args.iter().any(|a| {
            a == "set" || a == "edit" || a == "add" || a == "remove" || a == "rm" || a == "delete"
        })
}

fn find_args_are_read_only(args: &[String]) -> bool {
    !args.iter().any(|a| {
        matches!(
            a.as_str(),
            "-delete"
                | "-exec"
                | "-execdir"
                | "-ok"
                | "-okdir"
                | "-fprint"
                | "-fprint0"
                | "-fprintf"
                | "-fls"
        )
    })
}

fn sed_args_are_read_only(args: &[String]) -> bool {
    if args
        .iter()
        .any(|a| a == "-i" || a == "--in-place" || a.starts_with("-i"))
    {
        return false;
    }
    !args.iter().any(|a| a.contains("w ") || a.contains(">"))
}

fn git_args_are_read_only(args: &[String]) -> bool {
    let Some(first) = args.first().map(String::as_str) else {
        return false;
    };
    if !GIT_READ_SUBCOMMANDS.contains(&first) {
        return false;
    }
    let rest = &args[1..];
    match first {
        "config" => {
            rest.iter().any(|a| {
                a == "--get"
                    || a == "--get-all"
                    || a == "--get-regexp"
                    || a == "--list"
                    || a == "-l"
            }) && !rest
                .iter()
                .any(|a| a == "set" || a == "--add" || a == "--unset" || a == "--unset-all")
        }
        "branch" => !rest.iter().any(|a| {
            a == "-d"
                || a == "-D"
                || a == "--delete"
                || a == "-m"
                || a == "-M"
                || a == "--move"
                || a == "-c"
                || a == "--copy"
                || a == "--set-upstream-to"
                || a == "-u"
        }),
        "tag" => !rest
            .iter()
            .any(|a| a == "-d" || a == "--delete" || a == "-s" || a == "-a"),
        "remote" => matches!(
            rest.first().map(String::as_str),
            None | Some("show") | Some("get-url") | Some("-v") | Some("--verbose")
        ),
        "stash" => matches!(
            rest.first().map(String::as_str),
            None | Some("list") | Some("show")
        ),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn flat_allowlist_examples() {
        for prog in ["ls", "cat", "pwd", "whoami", "uname", "rg", "grep"] {
            assert_eq!(
                is_read_only_invocation(prog, &s(&[])),
                Some(RiskLevel::ReadOnly),
                "expected {prog} to be read-only"
            );
        }
        assert_eq!(
            is_read_only_invocation("ls", &s(&["-la", "/tmp"])),
            Some(RiskLevel::ReadOnly)
        );
    }

    #[test]
    fn shells_are_blocked() {
        for prog in ["bash", "sh", "zsh", "fish", "dash"] {
            assert_eq!(is_read_only_invocation(prog, &s(&["-c", "ls"])), None);
        }
    }

    #[test]
    fn paths_are_rejected() {
        assert_eq!(is_read_only_invocation("/usr/bin/ls", &s(&[])), None);
        assert_eq!(is_read_only_invocation("./foo", &s(&[])), None);
    }

    #[test]
    fn find_with_delete_is_blocked() {
        assert_eq!(
            is_read_only_invocation("find", &s(&[".", "-name", "*.rs"])),
            Some(RiskLevel::ReadOnly)
        );
        assert_eq!(is_read_only_invocation("find", &s(&[".", "-delete"])), None);
        assert_eq!(
            is_read_only_invocation("find", &s(&[".", "-exec", "rm", "{}", ";"])),
            None
        );
    }

    #[test]
    fn crontab_only_l() {
        assert_eq!(
            is_read_only_invocation("crontab", &s(&["-l"])),
            Some(RiskLevel::ReadOnly)
        );
        assert_eq!(is_read_only_invocation("crontab", &s(&["-r"])), None);
        assert_eq!(is_read_only_invocation("crontab", &s(&[])), None);
    }

    #[test]
    fn git_subcommands() {
        for sub in [
            "status",
            "log",
            "diff",
            "show",
            "rev-parse",
            "ls-files",
            "ls-tree",
            "cat-file",
            "blame",
            "describe",
        ] {
            assert_eq!(
                is_read_only_invocation("git", &s(&[sub])),
                Some(RiskLevel::ReadOnly),
                "git {sub}"
            );
        }
        assert_eq!(is_read_only_invocation("git", &s(&["push"])), None);
        assert_eq!(is_read_only_invocation("git", &s(&["commit"])), None);
        assert_eq!(is_read_only_invocation("git", &s(&[])), None);
    }

    #[test]
    fn git_config_only_get() {
        assert_eq!(
            is_read_only_invocation("git", &s(&["config", "--get", "user.email"])),
            Some(RiskLevel::ReadOnly)
        );
        assert_eq!(
            is_read_only_invocation("git", &s(&["config", "user.email", "x@y"])),
            None
        );
    }

    #[test]
    fn jq_blocks_in_place() {
        assert_eq!(
            is_read_only_invocation("jq", &s(&[".", "x.json"])),
            Some(RiskLevel::ReadOnly)
        );
        assert_eq!(
            is_read_only_invocation("jq", &s(&["-i", ".", "x.json"])),
            None
        );
        assert_eq!(
            is_read_only_invocation("jq", &s(&["--in-place", ".", "x.json"])),
            None
        );
    }

    #[test]
    fn version_only_runtimes() {
        for prog in ["python", "python3", "node", "ruby", "java"] {
            assert_eq!(
                is_read_only_invocation(prog, &s(&["--version"])),
                Some(RiskLevel::ReadOnly)
            );
            assert_eq!(
                is_read_only_invocation(prog, &s(&["-V"])),
                Some(RiskLevel::ReadOnly)
            );
            assert_eq!(is_read_only_invocation(prog, &s(&["script.py"])), None);
        }
    }

    #[test]
    fn package_managers_query_only() {
        assert_eq!(
            is_read_only_invocation("npm", &s(&["list"])),
            Some(RiskLevel::ReadOnly)
        );
        assert_eq!(is_read_only_invocation("npm", &s(&["install"])), None);
        assert_eq!(
            is_read_only_invocation("cargo", &s(&["--version"])),
            Some(RiskLevel::ReadOnly)
        );
        assert_eq!(is_read_only_invocation("cargo", &s(&["build"])), None);
    }

    #[test]
    fn parse_claimed_risk_round_trip() {
        assert_eq!(parse_claimed_risk("read_only"), Some(RiskLevel::ReadOnly));
        assert_eq!(
            parse_claimed_risk("destructive"),
            Some(RiskLevel::Destructive)
        );
        assert_eq!(parse_claimed_risk("unknown"), Some(RiskLevel::Unknown));
        assert_eq!(parse_claimed_risk("nonsense"), None);
    }
}
