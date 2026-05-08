use crate::memory::InitAudit;
use crate::sysctx::{find_disk_for_path, format_bytes};
use std::path::PathBuf;

const PROBED_TOOLS: &[&str] = &[
    "git", "docker", "kubectl", "node", "python", "python3", "cargo", "rustc", "go", "brew",
    "make", "gcc", "clang", "ssh", "curl", "wget", "jq", "fd", "rg", "fzf",
];

pub struct DetectedTool {
    pub name: String,
    pub version: Option<String>,
}

pub struct Identity {
    host: String,
    os_name: String,
    os_version: String,
    arch: String,
    user: String,
    uid_suffix: String,
    home: String,
    shell: Option<String>,
}

pub struct Hardware {
    cpu_cores: usize,
    cpu_brand: String,
    ram_total_bytes: u64,
    ram_free_bytes: u64,
    home_disk_free_bytes: Option<u64>,
    home_disk_total_bytes: Option<u64>,
    load_avg_1min: Option<f64>,
}

pub struct MachineAudit {
    written_at: String,
    identity: Identity,
    hardware: Hardware,
    tooling: Vec<DetectedTool>,
}

impl MachineAudit {
    pub fn capture() -> Self {
        let written_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

        let identity = capture_identity();
        let hardware = capture_hardware(&identity.home);

        MachineAudit {
            written_at,
            identity,
            hardware,
            tooling: vec![],
        }
    }

    pub async fn capture_with_tooling() -> Self {
        let mut audit = Self::capture();
        audit.tooling = probe_tools().await;
        audit
    }

    pub fn render_markdown(&self) -> String {
        let mut out = String::new();

        out.push_str(&format!("# Machine audit — {}\n", self.written_at));
        out.push('\n');
        out.push_str("## Identity\n");
        out.push_str(&format!("host: {}\n", self.identity.host));
        out.push_str(&format!(
            "os: {} {} ({})\n",
            self.identity.os_name, self.identity.os_version, self.identity.arch
        ));
        let user_line = if self.identity.uid_suffix.is_empty() {
            format!("user: {}\n", self.identity.user)
        } else {
            format!(
                "user: {} ({})\n",
                self.identity.user, self.identity.uid_suffix
            )
        };
        out.push_str(&user_line);
        out.push_str(&format!("home: {}\n", self.identity.home));
        if let Some(shell) = &self.identity.shell {
            out.push_str(&format!("shell: {}\n", shell));
        }

        out.push('\n');
        out.push_str("## Hardware\n");
        out.push_str(&format!(
            "cpu: {} cores ({})\n",
            self.hardware.cpu_cores, self.hardware.cpu_brand
        ));
        out.push_str(&format!(
            "ram: {} total, {} free\n",
            format_bytes(self.hardware.ram_total_bytes),
            format_bytes(self.hardware.ram_free_bytes)
        ));
        if let (Some(free), Some(total)) = (
            self.hardware.home_disk_free_bytes,
            self.hardware.home_disk_total_bytes,
        ) {
            out.push_str(&format!(
                "disk on home: {} free / {}\n",
                format_bytes(free),
                format_bytes(total)
            ));
        }
        #[cfg(unix)]
        if let Some(load) = self.hardware.load_avg_1min {
            out.push_str(&format!("load (1 min): {:.2}\n", load));
        }

        out.push('\n');
        out.push_str("## Tooling detected\n");
        let mut sorted: Vec<&DetectedTool> = self.tooling.iter().collect();
        sorted.sort_by(|a, b| a.name.cmp(&b.name));
        if sorted.is_empty() {
            out.push_str("(none)\n");
        } else {
            let parts: Vec<String> = sorted
                .iter()
                .map(|t| match &t.version {
                    Some(v) => format!("{} {}", t.name, v),
                    None => t.name.clone(),
                })
                .collect();
            out.push_str(&parts.join(", "));
            out.push('\n');
        }

        out
    }

    pub fn render_short_summary(&self) -> String {
        let mut lines = Vec::new();
        lines.push("machine audit:".to_string());
        lines.push(format!("  host: {}", self.identity.host));
        lines.push(format!(
            "  os: {} {} ({})",
            self.identity.os_name, self.identity.os_version, self.identity.arch
        ));
        lines.push(format!("  user: {}", self.identity.user));
        lines.push(format!(
            "  cpu: {} cores ({})",
            self.hardware.cpu_cores, self.hardware.cpu_brand
        ));
        lines.push(format!(
            "  ram: {} total",
            format_bytes(self.hardware.ram_total_bytes)
        ));
        lines.push(format!(
            "  tooling: {} binaries detected",
            self.tooling.len()
        ));
        lines.join("\n")
    }

    pub fn into_init_audit(self) -> InitAudit {
        let summary_md = self.render_markdown();
        InitAudit {
            written_at: self.written_at,
            host: self.identity.host,
            os: format!(
                "{} {} {}",
                self.identity.os_name, self.identity.os_version, self.identity.arch
            ),
            kernel: sysinfo::System::kernel_version().unwrap_or_else(|| "unknown".into()),
            user: self.identity.user,
            home: self.identity.home,
            shell: self.identity.shell,
            summary_md,
        }
    }

    pub fn identity_host(&self) -> &str {
        &self.identity.host
    }

    pub fn identity_os(&self) -> String {
        format!(
            "{} {} {}",
            self.identity.os_name, self.identity.os_version, self.identity.arch
        )
    }

    pub fn identity_user(&self) -> &str {
        &self.identity.user
    }

    pub fn tooling_count(&self) -> usize {
        self.tooling.len()
    }

    #[cfg(test)]
    pub(crate) fn for_test(
        written_at: String,
        identity: Identity,
        hardware: Hardware,
        tooling: Vec<DetectedTool>,
    ) -> Self {
        Self {
            written_at,
            identity,
            hardware,
            tooling,
        }
    }
}

impl Identity {
    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn for_test(
        host: String,
        os_name: String,
        os_version: String,
        arch: String,
        user: String,
        uid_suffix: String,
        home: String,
        shell: Option<String>,
    ) -> Self {
        Self {
            host,
            os_name,
            os_version,
            arch,
            user,
            uid_suffix,
            home,
            shell,
        }
    }
}

impl Hardware {
    #[cfg(test)]
    pub(crate) fn for_test(
        cpu_cores: usize,
        cpu_brand: String,
        ram_total_bytes: u64,
        ram_free_bytes: u64,
        home_disk_free_bytes: Option<u64>,
        home_disk_total_bytes: Option<u64>,
        load_avg_1min: Option<f64>,
    ) -> Self {
        Self {
            cpu_cores,
            cpu_brand,
            ram_total_bytes,
            ram_free_bytes,
            home_disk_free_bytes,
            home_disk_total_bytes,
            load_avg_1min,
        }
    }
}

fn capture_identity() -> Identity {
    let host = sysinfo::System::host_name().unwrap_or_else(|| "unknown".into());
    let os_name = sysinfo::System::name().unwrap_or_else(|| "unknown".into());
    let os_version = sysinfo::System::os_version().unwrap_or_else(|| "unknown".into());
    let arch = sysinfo::System::cpu_arch().unwrap_or_else(|| "unknown".into());

    let user = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown".into());

    #[cfg(unix)]
    let uid_suffix = format!("uid {}", unsafe { libc::getuid() });
    #[cfg(not(unix))]
    let uid_suffix = String::new();

    let home = std::env::var("HOME")
        .ok()
        .or_else(|| {
            directories::UserDirs::new().map(|d| d.home_dir().to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "unknown".into());

    let shell = std::env::var("SHELL").ok();

    Identity {
        host,
        os_name,
        os_version,
        arch,
        user,
        uid_suffix,
        home,
        shell,
    }
}

fn capture_hardware(home: &str) -> Hardware {
    let mut sys = sysinfo::System::new();
    sys.refresh_cpu_all();
    sys.refresh_memory();

    let cpu_cores = sys.cpus().len();
    let cpu_brand = sys
        .cpus()
        .first()
        .map(|c| c.brand().to_string())
        .unwrap_or_else(|| "unknown".into());
    let ram_total_bytes = sys.total_memory();
    let ram_free_bytes = sys.available_memory();

    let home_path = PathBuf::from(home);
    let (home_disk_free_bytes, home_disk_total_bytes) = find_disk_for_path(&home_path);

    #[cfg(unix)]
    let load_avg_1min = Some(sysinfo::System::load_average().one);
    #[cfg(not(unix))]
    let load_avg_1min: Option<f64> = None;

    Hardware {
        cpu_cores,
        cpu_brand,
        ram_total_bytes,
        ram_free_bytes,
        home_disk_free_bytes,
        home_disk_total_bytes,
        load_avg_1min,
    }
}

async fn probe_single_tool(name: &str) -> Option<DetectedTool> {
    use std::time::Duration;
    use tokio::process::Command;
    use tokio::time::timeout;

    let result = timeout(
        Duration::from_secs(1),
        Command::new(name).arg("--version").output(),
    )
    .await;

    let output = match result {
        Ok(Ok(o)) => o,
        _ => return None,
    };

    let raw = if !output.stdout.is_empty() {
        String::from_utf8_lossy(&output.stdout).to_string()
    } else if !output.stderr.is_empty() {
        String::from_utf8_lossy(&output.stderr).to_string()
    } else {
        return None;
    };

    let first_line = raw.lines().next()?.trim().to_string();
    if first_line.is_empty() {
        return None;
    }

    let version = extract_version(&first_line);

    Some(DetectedTool {
        name: name.to_string(),
        version,
    })
}

fn extract_version(line: &str) -> Option<String> {
    use regex::Regex;
    // Lazy static would be ideal but we avoid extra deps; compile on each call (only ~20 calls total).
    let re = Regex::new(r"\d+\.\d+(\.\d+)?").ok()?;
    let m = re.find(line)?;
    let v = &line[m.start()..m.end()];
    Some(v.chars().take(32).collect())
}

async fn probe_tools() -> Vec<DetectedTool> {
    let mut tools = Vec::new();
    for &name in PROBED_TOOLS {
        if let Some(tool) = probe_single_tool(name).await {
            tools.push(tool);
        }
    }
    tools
}

/// Run auto-init if the memory DB has no init_audit and `no_autoinit` is false.
///
/// Returns `true` if init was performed, `false` otherwise.
pub async fn run_autoinit_if_needed(
    memory: &crate::memory::Memory,
    no_autoinit: bool,
) -> anyhow::Result<bool> {
    if no_autoinit {
        return Ok(false);
    }
    if memory.read_init_audit()?.is_some() {
        return Ok(false);
    }
    let audit = MachineAudit::capture_with_tooling().await;
    memory.write_init_audit(&audit.into_init_audit())?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_audit() -> MachineAudit {
        MachineAudit::for_test(
            "2026-05-08T10:42:00Z".into(),
            Identity::for_test(
                "macbook-pro.local".into(),
                "macOS".into(),
                "25.3.0".into(),
                "arm64".into(),
                "swoelffel".into(),
                "uid 501".into(),
                "/Users/swoelffel".into(),
                Some("/bin/zsh".into()),
            ),
            Hardware::for_test(
                10,
                "Apple M2 Pro".into(),
                32 * 1024 * 1024 * 1024,
                18 * 1024 * 1024 * 1024,
                Some(124 * 1024 * 1024 * 1024),
                Some(460 * 1024 * 1024 * 1024),
                Some(1.5),
            ),
            vec![
                DetectedTool {
                    name: "cargo".into(),
                    version: Some("1.78.0".into()),
                },
                DetectedTool {
                    name: "git".into(),
                    version: Some("2.45.0".into()),
                },
                DetectedTool {
                    name: "node".into(),
                    version: Some("20.11.1".into()),
                },
            ],
        )
    }

    #[test]
    fn render_markdown_has_expected_sections() {
        let audit = make_test_audit();
        let md = audit.render_markdown();

        assert!(md.contains("# Machine audit"), "missing H1");
        assert!(md.contains("## Identity"), "missing Identity section");
        assert!(md.contains("## Hardware"), "missing Hardware section");
        assert!(
            md.contains("## Tooling detected"),
            "missing Tooling section"
        );
    }

    #[test]
    fn render_markdown_identity_fields() {
        let audit = make_test_audit();
        let md = audit.render_markdown();
        assert!(md.contains("host: macbook-pro.local"));
        assert!(md.contains("os: macOS 25.3.0 (arm64)"));
        assert!(md.contains("user: swoelffel (uid 501)"));
        assert!(md.contains("home: /Users/swoelffel"));
        assert!(md.contains("shell: /bin/zsh"));
    }

    #[test]
    fn render_markdown_hardware_fields() {
        let audit = make_test_audit();
        let md = audit.render_markdown();
        assert!(md.contains("cpu: 10 cores (Apple M2 Pro)"));
        assert!(md.contains("ram: 32 GiB total, 18 GiB free"));
        assert!(md.contains("disk on home: 124 GiB free / 460 GiB"));
    }

    #[test]
    fn tooling_sorted_alphabetically() {
        let audit = make_test_audit();
        let md = audit.render_markdown();
        let tooling_line = md
            .lines()
            .skip_while(|l| !l.starts_with("## Tooling"))
            .nth(1)
            .expect("no tooling line");
        // cargo < git < node
        let cargo_pos = tooling_line.find("cargo").expect("cargo missing");
        let git_pos = tooling_line.find("git").expect("git missing");
        let node_pos = tooling_line.find("node").expect("node missing");
        assert!(cargo_pos < git_pos, "cargo should come before git");
        assert!(git_pos < node_pos, "git should come before node");
    }

    #[test]
    fn tool_with_no_version_renders_as_name_only() {
        let audit = MachineAudit::for_test(
            "2026-05-08T10:42:00Z".into(),
            Identity::for_test(
                "host".into(),
                "Linux".into(),
                "6.1".into(),
                "x86_64".into(),
                "alice".into(),
                String::new(),
                "/home/alice".into(),
                None,
            ),
            Hardware::for_test(
                4,
                "Intel".into(),
                8 * 1024 * 1024 * 1024,
                2 * 1024 * 1024 * 1024,
                None,
                None,
                None,
            ),
            vec![DetectedTool {
                name: "make".into(),
                version: None,
            }],
        );
        let md = audit.render_markdown();
        let tooling_line = md
            .lines()
            .skip_while(|l| !l.starts_with("## Tooling"))
            .nth(1)
            .expect("no tooling line");
        assert_eq!(
            tooling_line.trim(),
            "make",
            "should be name only, no trailing space"
        );
    }

    #[test]
    fn render_short_summary_line_count() {
        let audit = make_test_audit();
        let s = audit.render_short_summary();
        let count = s.lines().count();
        assert!(
            (5..=10).contains(&count),
            "summary should have 5-10 lines, got {}",
            count
        );
    }

    #[test]
    fn into_init_audit_populates_summary_md() {
        let audit = make_test_audit();
        let md_preview = audit.render_markdown();
        let init = audit.into_init_audit();
        assert_eq!(
            init.summary_md, md_preview,
            "summary_md must match render_markdown()"
        );
    }

    #[test]
    fn into_init_audit_fields() {
        let audit = make_test_audit();
        let init = audit.into_init_audit();
        assert_eq!(init.host, "macbook-pro.local");
        assert_eq!(init.user, "swoelffel");
        assert_eq!(init.home, "/Users/swoelffel");
        assert_eq!(init.shell, Some("/bin/zsh".into()));
    }

    #[test]
    fn extract_version_finds_semver() {
        assert_eq!(extract_version("git version 2.45.0"), Some("2.45.0".into()));
        assert_eq!(extract_version("node v20.11.1"), Some("20.11.1".into()));
        assert_eq!(extract_version("cargo 1.78.0"), Some("1.78.0".into()));
    }

    #[test]
    fn extract_version_returns_none_when_absent() {
        assert_eq!(extract_version("no version here"), None);
        assert_eq!(extract_version(""), None);
    }

    #[tokio::test]
    async fn probe_nonexistent_tool_returns_none() {
        let result = probe_single_tool("nonexistent_tool_xyz_12345").await;
        assert!(result.is_none(), "nonexistent tool should return None");
    }

    #[tokio::test]
    async fn probe_git_returns_version() {
        let result = probe_single_tool("git").await;
        assert!(result.is_some(), "git should be detected");
        let tool = result.unwrap();
        assert_eq!(tool.name, "git");
        assert!(tool.version.is_some(), "git version should be extractable");
        let v = tool.version.unwrap();
        assert!(!v.is_empty(), "git version should not be empty");
    }

    #[tokio::test]
    async fn run_autoinit_writes_audit_when_empty() {
        let memory = crate::memory::Memory::open_in_memory().unwrap();
        assert!(memory.read_init_audit().unwrap().is_none());
        let ran = run_autoinit_if_needed(&memory, false).await.unwrap();
        assert!(ran, "should have run autoinit");
        assert!(memory.read_init_audit().unwrap().is_some());
    }

    #[tokio::test]
    async fn run_autoinit_skips_when_no_autoinit_true() {
        let memory = crate::memory::Memory::open_in_memory().unwrap();
        let ran = run_autoinit_if_needed(&memory, true).await.unwrap();
        assert!(!ran, "should not have run autoinit");
        assert!(memory.read_init_audit().unwrap().is_none());
    }

    #[tokio::test]
    async fn run_autoinit_skips_when_audit_exists() {
        use crate::memory::InitAudit;
        let memory = crate::memory::Memory::open_in_memory().unwrap();
        memory
            .write_init_audit(&InitAudit {
                written_at: "2026-05-08T00:00:00Z".into(),
                host: "existing".into(),
                os: "Linux".into(),
                kernel: "6.1".into(),
                user: "alice".into(),
                home: "/home/alice".into(),
                shell: None,
                summary_md: "existing audit".into(),
            })
            .unwrap();
        let ran = run_autoinit_if_needed(&memory, false).await.unwrap();
        assert!(!ran, "should not overwrite existing audit");
        let got = memory.read_init_audit().unwrap().unwrap();
        assert_eq!(got.host, "existing", "existing audit should be unchanged");
    }
}
