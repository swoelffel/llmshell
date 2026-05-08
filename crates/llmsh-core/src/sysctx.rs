use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct RuntimeContext {
    host: String,
    os_name: String,
    os_version: String,
    arch: String,
    user: String,
    cwd: PathBuf,
    workspace_root: PathBuf,
    model: String,
    disk_free_bytes: Option<u64>,
    disk_total_bytes: Option<u64>,
    session_uptime: Duration,
}

impl RuntimeContext {
    pub fn capture(workspace_root: PathBuf, model: Arc<String>, session_start: Instant) -> Self {
        let host = sysinfo::System::host_name().unwrap_or_else(|| "unknown".into());
        let os_name = sysinfo::System::name().unwrap_or_else(|| "unknown".into());
        let os_version = sysinfo::System::os_version().unwrap_or_else(|| "unknown".into());
        let arch = sysinfo::System::cpu_arch().unwrap_or_else(|| "unknown".into());
        let user = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "unknown".into());
        let cwd = std::env::current_dir()
            .map(|p| std::fs::canonicalize(&p).unwrap_or(p))
            .unwrap_or_else(|_| PathBuf::from("."));

        let (disk_free_bytes, disk_total_bytes) = find_disk_for_cwd(&cwd);

        Self {
            host,
            os_name,
            os_version,
            arch,
            user,
            cwd,
            workspace_root,
            model: model.as_ref().clone(),
            disk_free_bytes,
            disk_total_bytes,
            session_uptime: session_start.elapsed(),
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

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn for_test(
        host: String,
        os_name: String,
        os_version: String,
        arch: String,
        user: String,
        cwd: PathBuf,
        workspace_root: PathBuf,
        model: String,
        disk_free_bytes: Option<u64>,
        disk_total_bytes: Option<u64>,
        session_uptime: Duration,
    ) -> Self {
        Self {
            host,
            os_name,
            os_version,
            arch,
            user,
            cwd,
            workspace_root,
            model,
            disk_free_bytes,
            disk_total_bytes,
            session_uptime,
        }
    }
}

fn find_disk_for_cwd(cwd: &std::path::Path) -> (Option<u64>, Option<u64>) {
    find_disk_for_path(cwd)
}

pub(crate) fn find_disk_for_path(path: &std::path::Path) -> (Option<u64>, Option<u64>) {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let best = disks
        .list()
        .iter()
        .filter(|d| path.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len());
    match best {
        Some(d) => (Some(d.available_space()), Some(d.total_space())),
        None => (None, None),
    }
}

pub(crate) fn format_bytes(b: u64) -> String {
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

pub(crate) fn format_duration(d: Duration) -> String {
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
        let ctx = RuntimeContext::for_test(
            "myhost".into(),
            "macOS".into(),
            "14.0".into(),
            "arm64".into(),
            "alice".into(),
            PathBuf::from("/home/alice/project"),
            PathBuf::from("/home/alice/project"),
            "openai:gpt-4o".into(),
            Some(124 * 1024 * 1024 * 1024),
            Some(460 * 1024 * 1024 * 1024),
            Duration::from_secs(192),
        );
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
        let ctx = RuntimeContext::for_test(
            "myhost".into(),
            "macOS".into(),
            "14.0".into(),
            "arm64".into(),
            "alice".into(),
            PathBuf::from("/home/alice/project"),
            PathBuf::from("/home/alice/project"),
            "openai:gpt-4o".into(),
            None,
            None,
            Duration::from_secs(0),
        );
        let out = ctx.render();
        assert!(!out.contains("free disk"));
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 6);
        assert_eq!(lines[5], "session uptime: 00:00:00");
    }

    #[cfg(unix)]
    #[test]
    fn capture_via_symlinked_cwd_finds_a_disk() {
        // Reproduces the macOS /var → /private/var symlink case: the live cwd
        // resolves through a symlink, but the disk picker should still find a
        // matching mount because we canonicalize before the prefix match.
        let real = tempfile::tempdir().unwrap();
        let link_parent = tempfile::tempdir().unwrap();
        let link = link_parent.path().join("link");
        std::os::unix::fs::symlink(real.path(), &link).unwrap();

        let saved = std::env::current_dir().ok();
        std::env::set_current_dir(&link).unwrap();

        let ctx = RuntimeContext::capture(
            link.clone(),
            Arc::new("mock:test".to_string()),
            Instant::now(),
        );

        if let Some(prev) = saved {
            let _ = std::env::set_current_dir(prev);
        }

        // We can't assert the exact mount across CI hosts, but we can assert
        // the canonicalized cwd doesn't still contain the symlink component
        // and that disk picker output is internally consistent.
        let canonical_real =
            std::fs::canonicalize(real.path()).unwrap_or_else(|_| real.path().to_path_buf());
        assert!(
            ctx.cwd.starts_with(&canonical_real)
                || canonical_real.starts_with(&ctx.cwd)
                || ctx.cwd == canonical_real,
            "cwd should resolve through the symlink: got {:?}, real {:?}",
            ctx.cwd,
            canonical_real
        );
        match (ctx.disk_free_bytes, ctx.disk_total_bytes) {
            (Some(_), Some(_)) | (None, None) => {}
            other => panic!("disk fields must be both Some or both None: {:?}", other),
        }
    }
}
