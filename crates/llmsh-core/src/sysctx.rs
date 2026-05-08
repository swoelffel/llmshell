use std::path::PathBuf;
use std::time::{Duration, Instant};

pub struct RuntimeContextInput {
    pub workspace_root: PathBuf,
    pub model: String,
    pub session_start: Instant,
}

pub struct RuntimeContext {
    pub host: String,
    pub os_name: String,
    pub os_version: String,
    pub arch: String,
    pub kernel: String,
    pub user: String,
    pub cwd: PathBuf,
    pub workspace_root: PathBuf,
    pub model: String,
    pub disk_free_bytes: Option<u64>,
    pub disk_total_bytes: Option<u64>,
    pub session_uptime: Duration,
}

impl RuntimeContext {
    pub fn capture(input: RuntimeContextInput) -> Self {
        let host = sysinfo::System::host_name().unwrap_or_else(|| "unknown".into());
        let os_name = sysinfo::System::name().unwrap_or_else(|| "unknown".into());
        let os_version = sysinfo::System::os_version().unwrap_or_else(|| "unknown".into());
        let arch = sysinfo::System::cpu_arch().unwrap_or_else(|| "unknown".into());
        let kernel = sysinfo::System::kernel_version().unwrap_or_else(|| "unknown".into());
        let user = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "unknown".into());
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

        let (disk_free_bytes, disk_total_bytes) = find_disk_for_cwd(&cwd);

        let session_uptime = input.session_start.elapsed();

        Self {
            host,
            os_name,
            os_version,
            arch,
            kernel,
            user,
            cwd,
            workspace_root: input.workspace_root,
            model: input.model,
            disk_free_bytes,
            disk_total_bytes,
            session_uptime,
        }
    }

    pub fn render(&self) -> String {
        let mut lines = Vec::new();
        lines.push(format!(
            "host: {} ({} {} {})",
            self.host, self.os_name, self.os_version, self.arch
        ));
        lines.push(format!("user: {}", self.user));
        lines.push(format!("cwd: {}", self.cwd.display()));
        lines.push(format!("workspace_root: {}", self.workspace_root.display()));
        lines.push(format!("model: {}", self.model));
        if let (Some(free), Some(total)) = (self.disk_free_bytes, self.disk_total_bytes) {
            lines.push(format!(
                "free disk on cwd: {} / {}",
                format_bytes(free),
                format_bytes(total)
            ));
        }
        lines.push(format!(
            "session uptime: {}",
            format_duration(self.session_uptime)
        ));
        lines.join("\n")
    }
}

fn find_disk_for_cwd(cwd: &std::path::Path) -> (Option<u64>, Option<u64>) {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let best = disks
        .list()
        .iter()
        .filter(|d| cwd.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len());
    match best {
        Some(d) => (Some(d.available_space()), Some(d.total_space())),
        None => (None, None),
    }
}

pub fn format_bytes(b: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;
    const GIB: u64 = 1024 * MIB;
    const TIB: u64 = 1024 * GIB;

    if b >= TIB {
        format!("{} TiB", b / TIB)
    } else if b >= GIB {
        format!("{} GiB", b / GIB)
    } else if b >= MIB {
        format!("{} MiB", b / MIB)
    } else if b >= KIB {
        format!("{} KiB", b / KIB)
    } else {
        format!("{} B", b)
    }
}

pub fn format_duration(d: Duration) -> String {
    let total_secs = d.as_secs();
    let days = total_secs / 86400;
    let rem = total_secs % 86400;
    let hours = rem / 3600;
    let mins = (rem % 3600) / 60;
    let secs = rem % 60;
    if days > 0 {
        format!("{}d {:02}:{:02}:{:02}", days, hours, mins, secs)
    } else {
        format!("{:02}:{:02}:{:02}", hours, mins, secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn format_bytes_zero() {
        assert_eq!(format_bytes(0), "0 B");
    }

    #[test]
    fn format_bytes_below_kib() {
        assert_eq!(format_bytes(1023), "1023 B");
    }

    #[test]
    fn format_bytes_exactly_kib() {
        assert_eq!(format_bytes(1024), "1 KiB");
    }

    #[test]
    fn format_bytes_exactly_mib() {
        assert_eq!(format_bytes(1024 * 1024), "1 MiB");
    }

    #[test]
    fn format_bytes_exactly_gib() {
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1 GiB");
    }

    #[test]
    fn format_bytes_124_gib() {
        assert_eq!(format_bytes(124 * 1024 * 1024 * 1024), "124 GiB");
    }

    #[test]
    fn format_duration_zero() {
        assert_eq!(format_duration(Duration::from_secs(0)), "00:00:00");
    }

    #[test]
    fn format_duration_61s() {
        assert_eq!(format_duration(Duration::from_secs(61)), "00:01:01");
    }

    #[test]
    fn format_duration_3661s() {
        assert_eq!(format_duration(Duration::from_secs(3661)), "01:01:01");
    }

    #[test]
    fn format_duration_25h() {
        assert_eq!(
            format_duration(Duration::from_secs(25 * 3600)),
            "1d 01:00:00"
        );
    }

    #[test]
    fn render_with_disk_info() {
        let ctx = RuntimeContext {
            host: "myhost".into(),
            os_name: "macOS".into(),
            os_version: "14.0".into(),
            arch: "arm64".into(),
            kernel: "Darwin".into(),
            user: "alice".into(),
            cwd: PathBuf::from("/home/alice/project"),
            workspace_root: PathBuf::from("/home/alice/project"),
            model: "openai:gpt-4o".into(),
            disk_free_bytes: Some(124 * 1024 * 1024 * 1024),
            disk_total_bytes: Some(460 * 1024 * 1024 * 1024),
            session_uptime: Duration::from_secs(192),
        };
        let out = ctx.render();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "host: myhost (macOS 14.0 arm64)");
        assert_eq!(lines[1], "user: alice");
        assert_eq!(lines[2], "cwd: /home/alice/project");
        assert_eq!(lines[3], "workspace_root: /home/alice/project");
        assert_eq!(lines[4], "model: openai:gpt-4o");
        assert_eq!(lines[5], "free disk on cwd: 124 GiB / 460 GiB");
        assert_eq!(lines[6], "session uptime: 00:03:12");
    }

    #[test]
    fn render_without_disk_info() {
        let ctx = RuntimeContext {
            host: "myhost".into(),
            os_name: "macOS".into(),
            os_version: "14.0".into(),
            arch: "arm64".into(),
            kernel: "Darwin".into(),
            user: "alice".into(),
            cwd: PathBuf::from("/home/alice/project"),
            workspace_root: PathBuf::from("/home/alice/project"),
            model: "openai:gpt-4o".into(),
            disk_free_bytes: None,
            disk_total_bytes: None,
            session_uptime: Duration::from_secs(0),
        };
        let out = ctx.render();
        assert!(!out.contains("free disk"));
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 6);
        assert_eq!(lines[5], "session uptime: 00:00:00");
    }
}
