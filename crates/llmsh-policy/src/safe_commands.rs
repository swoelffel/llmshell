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
    if program.contains('/') {
        return None;
    }

    // Reject immediately if any argument contains a raw shell metacharacter.
    // SHELLS are handled separately below via `extract_shell_payload`, which
    // scans the `-c PAYLOAD` string for metachars itself. For non-shell
    // programs the OS execvp path is safe IF arguments are clean; an arg like
    // "inject|payload" would be benign to the OS but indicates an injection
    // attempt or confused caller — either way it is not provably read-only.
    if !SHELLS.contains(&program)
        && args
            .iter()
            .any(|a| a.chars().any(|c| SHELL_METACHARS.contains(&c)))
    {
        return None;
    }

    if SHELLS.contains(&program) {
        let (inner_prog, inner_args) = extract_shell_payload(program, args)?;
        if SHELLS.contains(&inner_prog.as_str()) || META_EXEC.contains(&inner_prog.as_str()) {
            return None;
        }
        return is_read_only_invocation(&inner_prog, &inner_args);
    }
    if META_EXEC.contains(&program) {
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
    fn shells_are_blocked() {
        // Shells without a safe `-c <payload>` form are still blocked.
        for prog in ["bash", "sh", "zsh", "fish", "dash"] {
            // Interactive / no args
            assert_eq!(
                is_read_only_invocation(prog, &s(&[])),
                None,
                "{prog} with no args"
            );
            // Unsafe payload (pipe metachar)
            assert_eq!(
                is_read_only_invocation(prog, &s(&["-c", "ls | grep foo"])),
                None,
                "{prog} -c 'ls | grep foo'"
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
    fn deshell_refuses_metacharacters() {
        for payload in [
            "ls | grep foo",
            "echo $HOME",
            "ls *.txt",
            "cat /etc/hosts > /tmp/x",
            "ls; rm foo",
            "ls && pwd",
            "ls\nrm",
            "echo `whoami`",
            "(ls)",
            "{ ls; }",
        ] {
            assert_eq!(
                is_read_only_invocation("bash", &s(&["-c", payload])),
                None,
                "payload should be refused: {payload}"
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
