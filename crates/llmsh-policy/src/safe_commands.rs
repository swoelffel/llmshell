//! Deterministic classifier for `run_process` invocations. Returns
//! `Some(RiskLevel::ReadOnly)` only when the (program, args) pair is
//! provably read-only. Anything ambiguous returns `None` so the caller
//! falls back to the existing `RiskLevel::Unknown` → `Confirm` flow.
//!
//! The richer entry point [`classify_invocation`] returns a
//! [`ClassificationReason`] when the verdict is not `ReadOnly`, so the
//! confirmation gate can surface *why* a command was left unclassified.

use crate::types::{ClassificationReason, RiskLevel};

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
    "findmnt",
    "mountpoint",
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
    "nproc",
    "arch",
    "getent",
    // Process inspection (no kill/-signal)
    "ps",
    "pgrep",
    "lsof",
    // macOS system info / hardware inspection
    "system_profiler",
    "sw_vers",
    "ioreg",
    "nettop",
    "scutil",
    "csrutil",
    "vm_stat",
    "iostat",
    // Linux hardware / system inspection
    "lscpu",
    "lsmem",
    "lsblk",
    "lspci",
    "lsusb",
    "lshw",
    "lsmod",
    "lsipc",
    "lsns",
    "free",
    // Distro package query (mutation-free tools, distinct from `dpkg`/`rpm`
    // which have install/remove subcommands).
    "dpkg-query",
    // Security audit scanners (no mutations to the host)
    "apparmor_status",
    "aa-status",
    "chkrootkit",
    "rkhunter",
    // Login records / active users
    "last",
    "w",
    "who",
    "users",
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

const SHELL_METACHARS: &[char] = &['|', '&', ';', '>', '<', '$', '`', '(', ')', '{', '}', '\n'];

/// Returns `Some((inner_program, inner_args))` only when `program ∈ SHELLS`,
/// `args` is exactly `["-c", payload]`, and `payload` is free of shell
/// metacharacters and parses cleanly via `shlex::split`. Otherwise `None`.
fn extract_shell_payload(program: &str, args: &[String]) -> Option<(String, Vec<String>)> {
    if !SHELLS.contains(&program) {
        return None;
    }
    // Accept `-c PAYLOAD [positional...]`: anything after PAYLOAD becomes
    // `$0`, `$1`, ... inside the body. Classification is driven by PAYLOAD
    // itself, which we still scan for metacharacters below.
    if args.len() < 2 || args[0] != "-c" {
        return None;
    }
    let payload = &args[1];
    if payload.chars().any(|c| SHELL_METACHARS.contains(&c)) {
        return None;
    }
    let mut tokens = shlex::split(payload)?;
    if tokens.is_empty() {
        return None;
    }
    if tokens[0].contains('=') {
        return None;
    }
    if tokens
        .iter()
        .any(|t| t.starts_with('*') || t.starts_with('?') || t.starts_with('['))
    {
        return None;
    }
    let inner_prog = tokens.remove(0);
    Some((inner_prog, tokens))
}

pub fn is_read_only_invocation(program: &str, args: &[String]) -> Option<RiskLevel> {
    classify_invocation(program, args).ok()
}

/// Rich version of [`is_read_only_invocation`]. On success returns the
/// concrete [`RiskLevel`]; on failure returns the [`ClassificationReason`]
/// that surfaces in the confirmation prompt and audit log.
pub fn classify_invocation(
    program: &str,
    args: &[String],
) -> Result<RiskLevel, ClassificationReason> {
    classify_invocation_inner(program, args, false)
}

/// Internal entry point. When `args_prevalidated` is true, the caller
/// guarantees that any shell metacharacters present in `args` are literal
/// (they came out of the quoting-aware lexer) and the up-front
/// SHELL_METACHARS guard is skipped. Used by `classify_shell_payload` to
/// recurse into pipeline segments.
fn classify_invocation_inner(
    program: &str,
    args: &[String],
    args_prevalidated: bool,
) -> Result<RiskLevel, ClassificationReason> {
    if program.contains('/') {
        return Err(ClassificationReason::AbsoluteOrRelativePath);
    }

    // Reject immediately if any argument contains a raw shell metacharacter.
    // SHELLS are handled separately below via the shell-payload analyzers.
    // For non-shell programs the OS execvp path is safe IF arguments are
    // clean; an arg like "inject|payload" would be benign to the OS but
    // indicates an injection attempt or confused caller.
    if !args_prevalidated
        && !SHELLS.contains(&program)
        && args
            .iter()
            .any(|a| a.chars().any(|c| SHELL_METACHARS.contains(&c)))
    {
        return Err(ClassificationReason::UnsafeArgument);
    }

    if SHELLS.contains(&program) {
        if args.len() < 2 || args[0] != "-c" {
            return Err(ClassificationReason::NotShellDashCForm);
        }
        // Try the strict deshell first (cheap, covers payloads without
        // metacharacters).
        if let Some((inner_prog, inner_args)) = extract_shell_payload(program, args) {
            if SHELLS.contains(&inner_prog.as_str()) {
                return Err(ClassificationReason::NestedShellWrapping);
            }
            if META_EXEC.contains(&inner_prog.as_str()) {
                return Err(ClassificationReason::ProgramNotAllowlisted);
            }
            return classify_invocation(&inner_prog, &inner_args);
        }
        // Fall back to the pipeline / safe-redirection analyzer (A + B).
        return classify_shell_payload(&args[1]);
    }
    if META_EXEC.contains(&program) {
        return Err(ClassificationReason::ProgramNotAllowlisted);
    }

    if ALWAYS_SAFE.contains(&program) {
        return Ok(RiskLevel::ReadOnly);
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
        "dscl" => dscl_args_are_read_only(args),
        "pfctl" => pfctl_args_are_read_only(args),
        "defaults" => defaults_args_are_read_only(args),
        "launchctl" => launchctl_args_are_read_only(args),
        "networksetup" => networksetup_args_are_read_only(args),
        "sysctl" => sysctl_args_are_read_only(args),
        "ip" => ip_args_are_read_only(args),
        "ss" => ss_args_are_read_only(args),
        "ufw" => ufw_args_are_read_only(args),
        "systemctl" => systemctl_args_are_read_only(args),
        "journalctl" => journalctl_args_are_read_only(args),
        "iptables" | "ip6tables" => iptables_args_are_read_only(args),
        "nft" => nft_args_are_read_only(args),
        "firewall-cmd" => firewall_cmd_args_are_read_only(args),
        "mount" => mount_args_are_read_only(args),
        "dmesg" => dmesg_args_are_read_only(args),
        _ => return Err(ClassificationReason::ProgramNotAllowlisted),
    };
    if safe {
        Ok(RiskLevel::ReadOnly)
    } else {
        Err(ClassificationReason::UnsafeArgument)
    }
}

/// Analyse a `bash -c "<payload>"` body that contains shell metacharacters
/// the strict deshell rejected. Accepts pipelines (`|`), short-circuit
/// chains (`&&`, `||`) and a small set of safe output redirections.
///
/// Rejects (returns `Err`):
/// - command substitution (`$(…)`, backticks) and process substitution
///   (`<(…)`, `>(…)`),
/// - variable expansion (`$`),
/// - `;` sequence and `&` background,
/// - globs (`*`, `?`, `[`) at any position,
/// - redirection targets other than `/dev/null` or `/tmp/<simple-name>`,
/// - any pipeline segment whose program is not classified as read-only,
/// - nested shells.
fn classify_shell_payload(payload: &str) -> Result<RiskLevel, ClassificationReason> {
    if payload.contains(';') {
        return Err(ClassificationReason::SequenceOrBackground);
    }
    if payload.contains('\n') || payload.contains('`') {
        return Err(ClassificationReason::CommandSubstitution);
    }
    if payload.contains("$(") || payload.contains("<(") || payload.contains(">(") {
        return Err(ClassificationReason::CommandSubstitution);
    }
    if payload.contains('$') {
        return Err(ClassificationReason::VariableExpansion);
    }
    if payload.contains('{')
        || payload.contains('}')
        || payload.contains('(')
        || payload.contains(')')
    {
        // Subshell / brace group.
        return Err(ClassificationReason::CommandSubstitution);
    }

    let lexemes = crate::shell_lex::lex(payload).map_err(|e| match e {
        crate::shell_lex::LexError::UnterminatedQuote
        | crate::shell_lex::LexError::DanglingEscape
        | crate::shell_lex::LexError::UnsupportedConstruct => {
            ClassificationReason::UnparsableShellPayload
        }
    })?;
    if lexemes.is_empty() {
        return Err(ClassificationReason::UnparsableShellPayload);
    }

    let mut segments: Vec<Vec<String>> = vec![Vec::new()];
    let mut i = 0;
    while i < lexemes.len() {
        match &lexemes[i] {
            crate::shell_lex::Lexeme::Op(op) => {
                use crate::shell_lex::Operator::*;
                match op {
                    Pipe | AndIf | OrIf => {
                        if segments.last().map(|s| s.is_empty()).unwrap_or(true) {
                            return Err(ClassificationReason::UnparsableShellPayload);
                        }
                        segments.push(Vec::new());
                    }
                    Background | Semicolon => {
                        return Err(ClassificationReason::SequenceOrBackground);
                    }
                    RedirOut | RedirAppend | RedirOutN(_) | RedirAppendN(_) | RedirAll
                    | RedirAllAppend => {
                        let target = match lexemes.get(i + 1) {
                            Some(crate::shell_lex::Lexeme::Word(t)) => &t.value,
                            _ => return Err(ClassificationReason::UnsafeRedirectionTarget),
                        };
                        if !is_safe_redirect_target(target) {
                            return Err(ClassificationReason::UnsafeRedirectionTarget);
                        }
                        i += 1;
                    }
                    RedirIn => match lexemes.get(i + 1) {
                        Some(crate::shell_lex::Lexeme::Word(_)) => {
                            i += 1;
                        }
                        _ => return Err(ClassificationReason::UnparsableShellPayload),
                    },
                    FdDup(_) => { /* harmless FD duplication */ }
                }
            }
            crate::shell_lex::Lexeme::Word(tok) => {
                // Le `|` à l'intérieur d'un token quoté est désormais préservé
                // par le lexer comme caractère littéral — plus de garde-fou
                // `tok.contains('|')` ici.
                let value = &tok.value;
                if let Some(rest) = strip_redirect_prefix(value) {
                    if !is_safe_redirect_target(rest) {
                        return Err(ClassificationReason::UnsafeRedirectionTarget);
                    }
                } else if value.contains('*') || value.contains('?') || value.starts_with('[') {
                    return Err(ClassificationReason::GlobNotResolved);
                } else {
                    segments
                        .last_mut()
                        .expect("segments is never empty")
                        .push(value.clone());
                }
            }
        }
        i += 1;
    }

    for seg in &segments {
        if seg.is_empty() {
            return Err(ClassificationReason::UnparsableShellPayload);
        }
        let prog = seg[0].as_str();
        if prog.contains('=') {
            return Err(ClassificationReason::VariableExpansion);
        }
        if SHELLS.contains(&prog) {
            return Err(ClassificationReason::NestedShellWrapping);
        }
        if META_EXEC.contains(&prog) {
            return Err(ClassificationReason::ProgramNotAllowlisted);
        }
        let inner_args = seg[1..].to_vec();
        // Args came out of the quoting-aware lexer: any `|`/`&`/etc inside
        // them is a literal character (quoted or backslash-escaped), not a
        // shell operator. Skip the up-front SHELL_METACHARS guard.
        match classify_invocation_inner(prog, &inner_args, true) {
            Ok(RiskLevel::ReadOnly) => {}
            Ok(_) | Err(_) => return Err(ClassificationReason::UnsafePipelineSegment),
        }
    }
    Ok(RiskLevel::ReadOnly)
}

fn strip_redirect_prefix(token: &str) -> Option<&str> {
    for prefix in ["&>>", "&>", "2>>", "1>>", "2>", "1>", ">>", ">"] {
        if let Some(rest) = token.strip_prefix(prefix) {
            if rest.is_empty() {
                return None;
            }
            return Some(rest);
        }
    }
    None
}

fn is_safe_redirect_target(target: &str) -> bool {
    if target == "/dev/null" {
        return true;
    }
    if let Some(rest) = target.strip_prefix("/tmp/") {
        return !rest.is_empty()
            && !rest.contains("..")
            && rest
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/'));
    }
    false
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

fn dscl_args_are_read_only(args: &[String]) -> bool {
    const READ_SUBS: &[&str] = &[
        "list", "read", "readall", "search", "-list", "-read", "-readall", "-search",
    ];
    args.iter().any(|a| READ_SUBS.contains(&a.as_str()))
}

fn pfctl_args_are_read_only(args: &[String]) -> bool {
    let Some(first) = args.first().map(String::as_str) else {
        return false;
    };
    if !first.starts_with("-s") {
        return false;
    }
    !args.iter().any(|a| {
        matches!(
            a.as_str(),
            "-e" | "-d" | "-f" | "-F" | "-k" | "-K" | "-N" | "-O" | "-q" | "-R" | "-T"
        )
    })
}

fn defaults_args_are_read_only(args: &[String]) -> bool {
    matches!(
        args.first().map(String::as_str),
        Some("read") | Some("read-type") | Some("domains") | Some("find") | Some("help")
    )
}

fn launchctl_args_are_read_only(args: &[String]) -> bool {
    matches!(
        args.first().map(String::as_str),
        Some("list")
            | Some("print")
            | Some("print-cache")
            | Some("print-disabled")
            | Some("blame")
            | Some("dumpstate")
            | Some("help")
            | Some("version")
    )
}

fn networksetup_args_are_read_only(args: &[String]) -> bool {
    let Some(first) = args.first().map(String::as_str) else {
        return false;
    };
    first.starts_with("-list") || first.starts_with("-get") || first.starts_with("-print")
}

fn sysctl_args_are_read_only(args: &[String]) -> bool {
    if args.is_empty() {
        return false;
    }
    !args
        .iter()
        .any(|a| a == "-w" || a == "-p" || a == "-f" || a.contains('='))
}

/// `ip <object> [show|list|get]` — anything else (add, del, set, change,
/// replace, flush) mutates network configuration. Leading inert flags
/// (`-4`, `-6`, `-s`, `-d`, `-h`, `-br`, `-j`/`--json`, `-c`/`--color`,
/// `-n`/`--netns <name>`) are skipped before the object lookup.
fn ip_args_are_read_only(args: &[String]) -> bool {
    const OBJECTS: &[&str] = &[
        "addr",
        "a",
        "address",
        "route",
        "r",
        "ro",
        "rou",
        "link",
        "l",
        "neigh",
        "n",
        "neighbor",
        "neighbour",
        "rule",
        "ru",
        "maddr",
        "mroute",
        "tunnel",
        "tun",
        "tcp_metrics",
        "tcpmetrics",
        "xfrm",
    ];
    let mut iter = args.iter().filter(|a| !a.starts_with('-')).peekable();
    let Some(object) = iter.next() else {
        return false;
    };
    if !OBJECTS.contains(&object.as_str()) {
        return false;
    }
    // Default action is `show` when omitted (e.g. `ip addr`).
    let action = iter.next().map(String::as_str).unwrap_or("show");
    matches!(action, "show" | "list" | "s" | "l" | "ls" | "get" | "g")
}

/// `ss` is a network socket inspector. The only documented mutating
/// switch is `-K` / `--kill` (terminates matching sockets); everything
/// else just queries kernel tables.
fn ss_args_are_read_only(args: &[String]) -> bool {
    !args.iter().any(|a| a == "-K" || a == "--kill")
}

/// `ufw status` / `ufw show <thing>` — anything else (enable, disable,
/// reset, default, allow, deny, reject, limit, delete, insert, route,
/// logging, app) mutates firewall state.
fn ufw_args_are_read_only(args: &[String]) -> bool {
    let first_pos = args
        .iter()
        .find(|a| !a.starts_with('-'))
        .map(String::as_str);
    matches!(first_pos, Some("status") | Some("show"))
}

/// `systemctl` read-only subcommands. Anything that starts/stops/enables
/// units mutates and is rejected.
fn systemctl_args_are_read_only(args: &[String]) -> bool {
    const READ_ONLY_SUBS: &[&str] = &[
        "status",
        "show",
        "cat",
        "list-units",
        "list-unit-files",
        "list-sockets",
        "list-timers",
        "list-jobs",
        "list-dependencies",
        "list-machines",
        "list-automounts",
        "list-paths",
        "is-active",
        "is-enabled",
        "is-failed",
        "is-system-running",
        "get-default",
        "show-environment",
    ];
    let first_pos = args
        .iter()
        .find(|a| !a.starts_with('-'))
        .map(String::as_str);
    matches!(first_pos, Some(s) if READ_ONLY_SUBS.contains(&s))
}

/// `journalctl` is read-only unless invoked with retention/flush flags.
fn journalctl_args_are_read_only(args: &[String]) -> bool {
    !args.iter().any(|a| {
        a == "--rotate"
            || a == "--flush"
            || a == "--sync"
            || a == "--relinquish-var"
            || a == "--smart-relinquish-var"
            || a.starts_with("--vacuum-")
    })
}

/// `iptables` / `ip6tables` are read-only when *any* listing flag is
/// present AND no mutating flag is present. A bare invocation requires
/// a flag, so empty args is rejected.
fn iptables_args_are_read_only(args: &[String]) -> bool {
    const MUTATING: &[&str] = &[
        "-A",
        "--append",
        "-D",
        "--delete",
        "-I",
        "--insert",
        "-R",
        "--replace",
        "-F",
        "--flush",
        "-X",
        "--delete-chain",
        "-Z",
        "--zero",
        "-N",
        "--new-chain",
        "-P",
        "--policy",
        "-E",
        "--rename-chain",
    ];
    if args.iter().any(|a| MUTATING.contains(&a.as_str())) {
        return false;
    }
    args.iter().any(|a| {
        a == "-L"
            || a == "--list"
            || a == "-S"
            || a == "--list-rules"
            || a == "-C"
            || a == "--check"
            || a.starts_with("-vL")
            || a.starts_with("-Ln")
    })
}

/// `nft list <…>` is read-only; `add`, `delete`, `flush`, `replace`,
/// `insert`, `create`, `rename`, `reset` mutate.
fn nft_args_are_read_only(args: &[String]) -> bool {
    let first_pos = args
        .iter()
        .find(|a| !a.starts_with('-'))
        .map(String::as_str);
    matches!(first_pos, Some("list"))
}

/// `firewall-cmd` is read-only when every flag is a query (`--list-…`,
/// `--get-…`, `--query-…`, `--info-…`, `--state`). `--permanent` is
/// inert by itself but commonly pairs with mutating flags, so reject any
/// args containing it to stay conservative.
fn firewall_cmd_args_are_read_only(args: &[String]) -> bool {
    if args.is_empty() {
        return false;
    }
    if args.iter().any(|a| a == "--permanent") {
        return false;
    }
    args.iter().all(|a| {
        a == "--state"
            || a.starts_with("--list-")
            || a.starts_with("--get-")
            || a.starts_with("--query-")
            || a.starts_with("--info-")
    })
}

/// Bare `mount` (no positional args) prints the mount table. As soon as
/// a source or target is passed it tries to mount, which mutates kernel
/// state.
fn mount_args_are_read_only(args: &[String]) -> bool {
    args.is_empty()
        || args
            .iter()
            .all(|a| a == "-l" || a == "-v" || a == "--show-labels")
}

/// `dmesg` is read-only unless `-c`/`-C`/`--clear`/`--read-clear` is
/// passed (those clear or read-and-clear the kernel ring buffer).
fn dmesg_args_are_read_only(args: &[String]) -> bool {
    !args
        .iter()
        .any(|a| a == "-c" || a == "-C" || a == "--clear" || a == "--read-clear")
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
    fn regression_pipe_alternation_in_grep_df() {
        // Real-world bash -c payload that classifies as Unknown instead of ReadOnly:
        // shlex strips the quotes around the regex, leaving tok.contains('|') unable
        // to distinguish operator from quoted regex alternation.
        let payload = "df -h | grep -E '^/dev|^Filesystem'";
        assert_eq!(
            is_read_only_invocation("bash", &s(&["-c", payload])),
            Some(RiskLevel::ReadOnly),
            "df -h piped to grep with regex alternation should be ReadOnly"
        );
    }

    #[test]
    fn regression_pipe_alternation_in_grep_sysctl() {
        // Real-world bash -c payload that classifies as Unknown instead of ReadOnly:
        // shlex strips the quotes around the regex, leaving tok.contains('|') unable
        // to distinguish operator from quoted regex alternation.
        let payload = "sysctl -a | grep -E 'kern.maxfiles|kern.maxfilesperproc|vm.swapusage'";
        assert_eq!(
            is_read_only_invocation("bash", &s(&["-c", payload])),
            Some(RiskLevel::ReadOnly),
            "sysctl -a piped to grep with regex alternation should be ReadOnly"
        );
    }

    #[test]
    fn phase3_glued_unquoted_pipe_is_read_only() {
        // `ls|wc -l` — pipe glued without spaces, both sides bare. The new
        // lexer splits the operator structurally; classification stays ReadOnly.
        assert_eq!(
            is_read_only_invocation("bash", &s(&["-c", "ls|wc -l"])),
            Some(RiskLevel::ReadOnly)
        );
    }

    #[test]
    fn phase3_quoted_pipe_preserved_as_literal() {
        // `echo 'a|b'` — pipe inside single quotes is literal; echo is in
        // ALWAYS_SAFE so the single segment classifies ReadOnly.
        assert_eq!(
            is_read_only_invocation("bash", &s(&["-c", "echo 'a|b'"])),
            Some(RiskLevel::ReadOnly)
        );
    }

    #[test]
    fn phase3_unterminated_quote_is_unparsable() {
        // `echo 'abc` — single quote never closed; lexer returns
        // UnterminatedQuote -> UnparsableShellPayload -> None.
        assert_eq!(
            is_read_only_invocation("bash", &s(&["-c", "echo 'abc"])),
            None
        );
    }

    #[test]
    fn phase3_heredoc_rejected() {
        // `cat <<EOF` — heredoc construct rejected as Unsupported by the
        // lexer, mapped to UnparsableShellPayload -> None.
        assert_eq!(
            is_read_only_invocation("bash", &s(&["-c", "cat <<EOF"])),
            None
        );
    }

    #[test]
    fn phase3_backslash_escape_pipe_is_literal() {
        // `echo a\|b` — the bare backslash escapes `|` so it's a literal
        // character inside one bare word; classification stays ReadOnly.
        assert_eq!(
            is_read_only_invocation("bash", &s(&["-c", r"echo a\|b"])),
            Some(RiskLevel::ReadOnly)
        );
    }

    #[test]
    fn linux_hardware_inspection_is_read_only() {
        for prog in [
            "lscpu",
            "lsmem",
            "lsblk",
            "lspci",
            "lsusb",
            "lshw",
            "lsmod",
            "lsipc",
            "lsns",
            "free",
            "dpkg-query",
            "apparmor_status",
            "aa-status",
            "chkrootkit",
            "rkhunter",
            "findmnt",
            "mountpoint",
            "nproc",
            "arch",
            "getent",
        ] {
            assert_eq!(
                is_read_only_invocation(prog, &s(&[])),
                Some(RiskLevel::ReadOnly),
                "expected {prog} to be read-only"
            );
        }
        // common flag combinations seen in audits
        assert_eq!(
            is_read_only_invocation("free", &s(&["-h"])),
            Some(RiskLevel::ReadOnly)
        );
        assert_eq!(
            is_read_only_invocation("lsblk", &s(&["-f"])),
            Some(RiskLevel::ReadOnly)
        );
        assert_eq!(
            is_read_only_invocation("dpkg-query", &s(&["-l"])),
            Some(RiskLevel::ReadOnly)
        );
    }

    #[test]
    fn ip_subcommand_is_read_only_only_on_show() {
        for obj in ["addr", "a", "address", "link", "route", "neigh", "rule"] {
            assert_eq!(
                is_read_only_invocation("ip", &s(&[obj])),
                Some(RiskLevel::ReadOnly),
                "ip {obj} (default action)"
            );
            assert_eq!(
                is_read_only_invocation("ip", &s(&[obj, "show"])),
                Some(RiskLevel::ReadOnly),
                "ip {obj} show"
            );
        }
        // Leading inert flags must be skipped.
        assert_eq!(
            is_read_only_invocation("ip", &s(&["-4", "addr"])),
            Some(RiskLevel::ReadOnly)
        );
        assert_eq!(
            is_read_only_invocation("ip", &s(&["-br", "-c", "link", "show"])),
            Some(RiskLevel::ReadOnly)
        );
        // Mutating subcommands rejected.
        assert_eq!(
            is_read_only_invocation("ip", &s(&["addr", "add", "10.0.0.1/24", "dev", "eth0"])),
            None
        );
        assert_eq!(
            is_read_only_invocation("ip", &s(&["link", "set", "eth0", "up"])),
            None
        );
        assert_eq!(
            is_read_only_invocation("ip", &s(&["route", "del", "default"])),
            None
        );
        // Unknown object rejected.
        assert_eq!(is_read_only_invocation("ip", &s(&["zoinx"])), None);
    }

    #[test]
    fn ss_is_read_only_unless_kill() {
        assert_eq!(
            is_read_only_invocation("ss", &s(&[])),
            Some(RiskLevel::ReadOnly)
        );
        assert_eq!(
            is_read_only_invocation("ss", &s(&["-tuln"])),
            Some(RiskLevel::ReadOnly)
        );
        assert_eq!(
            is_read_only_invocation("ss", &s(&["-K", "state", "all"])),
            None
        );
        assert_eq!(is_read_only_invocation("ss", &s(&["--kill"])), None);
    }

    #[test]
    fn ufw_only_status_and_show() {
        assert_eq!(
            is_read_only_invocation("ufw", &s(&["status"])),
            Some(RiskLevel::ReadOnly)
        );
        assert_eq!(
            is_read_only_invocation("ufw", &s(&["status", "verbose"])),
            Some(RiskLevel::ReadOnly)
        );
        assert_eq!(
            is_read_only_invocation("ufw", &s(&["show", "added"])),
            Some(RiskLevel::ReadOnly)
        );
        for sub in ["enable", "disable", "reset", "default", "allow", "deny"] {
            assert_eq!(
                is_read_only_invocation("ufw", &s(&[sub])),
                None,
                "ufw {sub}"
            );
        }
        assert_eq!(is_read_only_invocation("ufw", &s(&[])), None);
    }

    #[test]
    fn systemctl_read_only_subcommands() {
        for sub in [
            "status",
            "show",
            "cat",
            "list-units",
            "list-unit-files",
            "list-timers",
            "is-active",
            "is-enabled",
            "get-default",
        ] {
            assert_eq!(
                is_read_only_invocation("systemctl", &s(&[sub])),
                Some(RiskLevel::ReadOnly),
                "systemctl {sub}"
            );
        }
        assert_eq!(
            is_read_only_invocation("systemctl", &s(&["status", "ssh"])),
            Some(RiskLevel::ReadOnly)
        );
        for sub in [
            "start",
            "stop",
            "restart",
            "reload",
            "enable",
            "disable",
            "mask",
            "set-default",
        ] {
            assert_eq!(
                is_read_only_invocation("systemctl", &s(&[sub, "ssh"])),
                None,
                "systemctl {sub} ssh"
            );
        }
    }

    #[test]
    fn journalctl_read_only_unless_vacuum() {
        assert_eq!(
            is_read_only_invocation("journalctl", &s(&[])),
            Some(RiskLevel::ReadOnly)
        );
        assert_eq!(
            is_read_only_invocation("journalctl", &s(&["-u", "ssh", "-n", "50"])),
            Some(RiskLevel::ReadOnly)
        );
        assert_eq!(
            is_read_only_invocation("journalctl", &s(&["--vacuum-time=7d"])),
            None
        );
        assert_eq!(
            is_read_only_invocation("journalctl", &s(&["--rotate"])),
            None
        );
        assert_eq!(
            is_read_only_invocation("journalctl", &s(&["--flush"])),
            None
        );
    }

    #[test]
    fn iptables_list_only() {
        assert_eq!(
            is_read_only_invocation("iptables", &s(&["-L"])),
            Some(RiskLevel::ReadOnly)
        );
        assert_eq!(
            is_read_only_invocation("iptables", &s(&["-S"])),
            Some(RiskLevel::ReadOnly)
        );
        assert_eq!(
            is_read_only_invocation("iptables", &s(&["-L", "-n", "-v"])),
            Some(RiskLevel::ReadOnly)
        );
        assert_eq!(
            is_read_only_invocation("ip6tables", &s(&["--list-rules"])),
            Some(RiskLevel::ReadOnly)
        );
        // mutating
        assert_eq!(
            is_read_only_invocation("iptables", &s(&["-A", "INPUT", "-j", "DROP"])),
            None
        );
        assert_eq!(is_read_only_invocation("iptables", &s(&["-F"])), None);
        // bare invocation (no flag) → rejected
        assert_eq!(is_read_only_invocation("iptables", &s(&[])), None);
    }

    #[test]
    fn nft_only_list() {
        assert_eq!(
            is_read_only_invocation("nft", &s(&["list", "ruleset"])),
            Some(RiskLevel::ReadOnly)
        );
        for sub in ["add", "delete", "flush", "replace", "insert", "create"] {
            assert_eq!(
                is_read_only_invocation("nft", &s(&[sub])),
                None,
                "nft {sub}"
            );
        }
    }

    #[test]
    fn firewall_cmd_query_only() {
        assert_eq!(
            is_read_only_invocation("firewall-cmd", &s(&["--state"])),
            Some(RiskLevel::ReadOnly)
        );
        assert_eq!(
            is_read_only_invocation("firewall-cmd", &s(&["--list-all"])),
            Some(RiskLevel::ReadOnly)
        );
        assert_eq!(
            is_read_only_invocation("firewall-cmd", &s(&["--get-zones"])),
            Some(RiskLevel::ReadOnly)
        );
        // --permanent is conservative-rejected (commonly paired with mutation)
        assert_eq!(
            is_read_only_invocation("firewall-cmd", &s(&["--permanent", "--list-all"])),
            None
        );
        assert_eq!(
            is_read_only_invocation("firewall-cmd", &s(&["--add-service=http"])),
            None
        );
    }

    #[test]
    fn mount_only_bare() {
        assert_eq!(
            is_read_only_invocation("mount", &s(&[])),
            Some(RiskLevel::ReadOnly)
        );
        assert_eq!(
            is_read_only_invocation("mount", &s(&["-l"])),
            Some(RiskLevel::ReadOnly)
        );
        assert_eq!(
            is_read_only_invocation("mount", &s(&["/dev/sda1", "/mnt"])),
            None
        );
        assert_eq!(
            is_read_only_invocation("mount", &s(&["-o", "ro", "/dev/sda1", "/mnt"])),
            None
        );
    }

    #[test]
    fn dmesg_read_only_unless_clear() {
        assert_eq!(
            is_read_only_invocation("dmesg", &s(&[])),
            Some(RiskLevel::ReadOnly)
        );
        assert_eq!(
            is_read_only_invocation("dmesg", &s(&["-T"])),
            Some(RiskLevel::ReadOnly)
        );
        assert_eq!(is_read_only_invocation("dmesg", &s(&["-c"])), None);
        assert_eq!(is_read_only_invocation("dmesg", &s(&["-C"])), None);
        assert_eq!(is_read_only_invocation("dmesg", &s(&["--clear"])), None);
    }

    #[test]
    fn shells_without_dash_c_form_are_blocked() {
        // Shells without a `-c <payload>` form are still blocked, and any
        // payload whose inner program isn't read-only stays blocked even
        // when the pipeline parser engages.
        for prog in ["bash", "sh", "zsh", "fish", "dash"] {
            // Interactive / no args
            assert_eq!(
                is_read_only_invocation(prog, &s(&[])),
                None,
                "{prog} with no args"
            );
            // Pipe with a mutating segment must stay refused.
            assert_eq!(
                is_read_only_invocation(prog, &s(&["-c", "ls | rm foo"])),
                None,
                "{prog} -c 'ls | rm foo' must stay blocked"
            );
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

    #[test]
    fn deshell_engages_for_simple_read_only() {
        assert_eq!(
            is_read_only_invocation("bash", &s(&["-c", "ls"])),
            Some(RiskLevel::ReadOnly)
        );
        assert_eq!(
            is_read_only_invocation("bash", &s(&["-c", "ls -la /tmp"])),
            Some(RiskLevel::ReadOnly)
        );
        assert_eq!(
            is_read_only_invocation("sh", &s(&["-c", "grep TODO src/main.rs"])),
            Some(RiskLevel::ReadOnly)
        );
    }

    #[test]
    fn deshell_refuses_unsafe_metacharacters() {
        // These remain refused: variable expansion, globs, command
        // substitution, sequence/background, brace/subshell groups.
        for payload in [
            "echo $HOME",
            "ls *.txt",
            "ls; rm foo",
            "ls\nrm",
            "echo `whoami`",
            "(ls)",
            "{ ls; }",
            "ls & pwd",
            "ls $(rm bad)",
        ] {
            assert_eq!(
                is_read_only_invocation("bash", &s(&["-c", payload])),
                None,
                "payload should be refused: {payload}"
            );
        }
    }

    #[test]
    fn pipeline_of_read_only_segments_is_classified() {
        // A) pipes: every segment is read-only → whole pipeline is read-only.
        for payload in [
            "ls | wc -l",
            "find . -maxdepth 1 -type f | wc -l",
            "ls | sort | head",
            "grep TODO src/main.rs | wc -l",
            "ps aux | grep llmsh",
            "cat file.txt | jq '.x'",
        ] {
            assert_eq!(
                is_read_only_invocation("bash", &s(&["-c", payload])),
                Some(RiskLevel::ReadOnly),
                "expected read-only pipeline: {payload}"
            );
        }
    }

    #[test]
    fn short_circuit_chain_of_read_only_segments_is_classified() {
        assert_eq!(
            is_read_only_invocation("bash", &s(&["-c", "ls && pwd"])),
            Some(RiskLevel::ReadOnly)
        );
        assert_eq!(
            is_read_only_invocation("bash", &s(&["-c", "ls || true"])),
            Some(RiskLevel::ReadOnly)
        );
    }

    #[test]
    fn pipeline_with_mutating_segment_rejected() {
        // && to a mutating tool — must NOT be classified read-only.
        assert_eq!(
            is_read_only_invocation("bash", &s(&["-c", "ls && rm foo"])),
            None
        );
        assert_eq!(
            is_read_only_invocation("bash", &s(&["-c", "find . | xargs rm"])),
            None
        );
    }

    #[test]
    fn safe_redirection_to_dev_null_or_tmp_is_classified() {
        // B) outputs to /dev/null and /tmp/<simple-name> are safe.
        for payload in [
            "ls > /dev/null",
            "ls >/dev/null",
            "grep TODO src/main.rs 2>/dev/null",
            "ls > /tmp/out.txt",
            "ls >> /tmp/log",
            "find . | wc -l > /dev/null",
        ] {
            assert_eq!(
                is_read_only_invocation("bash", &s(&["-c", payload])),
                Some(RiskLevel::ReadOnly),
                "expected safe redirection accepted: {payload}"
            );
        }
    }

    #[test]
    fn unsafe_redirection_target_rejected() {
        for payload in [
            "ls > /etc/passwd",
            "cat /etc/hosts > /tmp/../etc/x",
            "ls > ~/.ssh/known_hosts",
            "ls > /home/u/file",
        ] {
            assert_eq!(
                is_read_only_invocation("bash", &s(&["-c", payload])),
                None,
                "payload must be refused: {payload}"
            );
        }
    }

    #[test]
    fn deshell_refuses_unsafe_inner_program() {
        assert_eq!(
            is_read_only_invocation("bash", &s(&["-c", "rm -rf /tmp"])),
            None
        );
    }

    #[test]
    fn deshell_recursion_bounded_to_one_level() {
        // Inner is itself a shell wrapper → second-level deshell is refused.
        assert_eq!(
            is_read_only_invocation("bash", &s(&["-c", "bash -c ls"])),
            None
        );
    }

    #[test]
    fn deshell_only_dash_c_form() {
        // Wrong arg shape: must start with -c PAYLOAD.
        assert_eq!(is_read_only_invocation("bash", &s(&["ls"])), None);
        assert_eq!(is_read_only_invocation("bash", &s(&["-c"])), None);
        assert_eq!(
            is_read_only_invocation("bash", &s(&["-x", "script.sh"])),
            None
        );
    }

    #[test]
    fn deshell_handles_bash_c_with_extra_positional_args() {
        // `bash -c PAYLOAD arg0 arg1` — positionals become $0,$1 inside the
        // body; classification still drives off PAYLOAD itself.
        assert_eq!(
            is_read_only_invocation("bash", &s(&["-c", "ls -la", "script_name", "extra"])),
            Some(RiskLevel::ReadOnly)
        );
    }

    #[test]
    fn deshell_supports_sh_dash_c() {
        assert_eq!(
            is_read_only_invocation("sh", &s(&["-c", "pwd"])),
            Some(RiskLevel::ReadOnly)
        );
    }

    #[test]
    fn deshell_supports_zsh_dash_c() {
        assert_eq!(
            is_read_only_invocation("zsh", &s(&["-c", "ls"])),
            Some(RiskLevel::ReadOnly)
        );
    }

    #[test]
    fn deshell_does_not_engage_for_sudo_wrapper() {
        // sudo is in META_EXEC, not SHELLS — it must not be deshelled.
        assert_eq!(
            is_read_only_invocation("sudo", &s(&["bash", "-c", "ls"])),
            None
        );
    }

    #[test]
    fn dscl_read_subcommands() {
        assert_eq!(
            is_read_only_invocation("dscl", &s(&[".", "list", "/Users"])),
            Some(RiskLevel::ReadOnly)
        );
        assert_eq!(
            is_read_only_invocation("dscl", &s(&[".", "read", "/Users/root"])),
            Some(RiskLevel::ReadOnly)
        );
        assert_eq!(
            is_read_only_invocation("dscl", &s(&[".", "-list", "/Users"])),
            Some(RiskLevel::ReadOnly)
        );
        assert_eq!(
            is_read_only_invocation("dscl", &s(&[".", "create", "/Users/x"])),
            None
        );
        assert_eq!(is_read_only_invocation("dscl", &s(&[])), None);
    }

    #[test]
    fn pfctl_status_only() {
        assert_eq!(
            is_read_only_invocation("pfctl", &s(&["-s", "info"])),
            Some(RiskLevel::ReadOnly)
        );
        assert_eq!(
            is_read_only_invocation("pfctl", &s(&["-sr"])),
            Some(RiskLevel::ReadOnly)
        );
        assert_eq!(is_read_only_invocation("pfctl", &s(&["-e"])), None);
        assert_eq!(is_read_only_invocation("pfctl", &s(&["-d"])), None);
        assert_eq!(
            is_read_only_invocation("pfctl", &s(&["-f", "/etc/pf.conf"])),
            None
        );
    }

    #[test]
    fn defaults_read_only() {
        assert_eq!(
            is_read_only_invocation("defaults", &s(&["read", "com.apple.dock"])),
            Some(RiskLevel::ReadOnly)
        );
        assert_eq!(
            is_read_only_invocation("defaults", &s(&["domains"])),
            Some(RiskLevel::ReadOnly)
        );
        assert_eq!(
            is_read_only_invocation("defaults", &s(&["write", "com.apple.dock", "x", "1"])),
            None
        );
    }

    #[test]
    fn launchctl_inspection_only() {
        for sub in ["list", "print", "print-disabled", "blame", "dumpstate"] {
            assert_eq!(
                is_read_only_invocation("launchctl", &s(&[sub])),
                Some(RiskLevel::ReadOnly),
                "launchctl {sub}"
            );
        }
        assert_eq!(
            is_read_only_invocation("launchctl", &s(&["load", "x.plist"])),
            None
        );
        assert_eq!(
            is_read_only_invocation("launchctl", &s(&["unload", "x.plist"])),
            None
        );
    }

    #[test]
    fn networksetup_read_only_flags() {
        assert_eq!(
            is_read_only_invocation("networksetup", &s(&["-listallhardwareports"])),
            Some(RiskLevel::ReadOnly)
        );
        assert_eq!(
            is_read_only_invocation("networksetup", &s(&["-getairportnetwork", "en0"])),
            Some(RiskLevel::ReadOnly)
        );
        assert_eq!(
            is_read_only_invocation("networksetup", &s(&["-printcommands"])),
            Some(RiskLevel::ReadOnly)
        );
        assert_eq!(
            is_read_only_invocation("networksetup", &s(&["-setairportpower", "en0", "off"])),
            None
        );
    }

    #[test]
    fn sysctl_read_only_no_write_flag() {
        assert_eq!(
            is_read_only_invocation("sysctl", &s(&["-a"])),
            Some(RiskLevel::ReadOnly)
        );
        assert_eq!(
            is_read_only_invocation("sysctl", &s(&["hw.memsize"])),
            Some(RiskLevel::ReadOnly)
        );
        assert_eq!(
            is_read_only_invocation("sysctl", &s(&["-w", "net.ipv4.ip_forward=1"])),
            None
        );
        assert_eq!(
            is_read_only_invocation("sysctl", &s(&["kernel.foo=1"])),
            None
        );
        assert_eq!(
            is_read_only_invocation("sysctl", &s(&["-p", "/etc/sysctl.conf"])),
            None
        );
    }

    #[test]
    fn macos_universal_read_only_tools() {
        for prog in [
            "system_profiler",
            "sw_vers",
            "ioreg",
            "nettop",
            "scutil",
            "csrutil",
            "vm_stat",
            "iostat",
            "last",
            "w",
            "who",
            "users",
        ] {
            assert_eq!(
                is_read_only_invocation(prog, &s(&[])),
                Some(RiskLevel::ReadOnly),
                "expected {prog} to be read-only"
            );
        }
    }
}
