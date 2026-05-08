use crate::event::AuditEvent;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

pub struct AuditWriter {
    path: PathBuf,
    file: Option<std::fs::File>,
    enabled: bool,
}

impl AuditWriter {
    pub fn open(dir: &Path, session_id: &str) -> anyhow::Result<Self> {
        ensure_dir_secure(dir)?;
        let path = dir.join(format!("{}.jsonl", session_id));
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        set_file_perms(&path, 0o600)?;
        Ok(Self { path, file: Some(file), enabled: true })
    }

    pub fn disabled() -> Self {
        Self { path: PathBuf::new(), file: None, enabled: false }
    }

    pub fn write(&mut self, ev: &AuditEvent) -> anyhow::Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let line = serde_json::to_string(ev)?;
        if let Some(f) = self.file.as_mut() {
            writeln!(f, "{}", line)?;
        }
        Ok(())
    }

    pub fn flush(&mut self) -> anyhow::Result<()> {
        if !self.enabled {
            return Ok(());
        }
        if let Some(f) = self.file.as_mut() {
            f.flush()?;
            f.sync_all()?;
        }
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn ensure_dir_secure(dir: &Path) -> anyhow::Result<()> {
    if !dir.exists() {
        std::fs::create_dir_all(dir)?;
    }
    set_file_perms(dir, 0o700)?;
    Ok(())
}

#[cfg(unix)]
fn set_file_perms(p: &Path, mode: u32) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let perms = std::fs::Permissions::from_mode(mode);
    std::fs::set_permissions(p, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn set_file_perms(_p: &Path, _mode: u32) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::now_iso;

    #[test]
    fn writes_jsonl() {
        let tmp = tempfile::tempdir().unwrap();
        let mut w = AuditWriter::open(tmp.path(), "sess-1").unwrap();
        w.write(&AuditEvent::SessionEnded { ts: now_iso(), reason: "test".into() }).unwrap();
        w.flush().unwrap();
        let s = std::fs::read_to_string(tmp.path().join("sess-1.jsonl")).unwrap();
        assert!(s.contains("session_ended"));
    }

    #[cfg(unix)]
    #[test]
    fn file_is_0600() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let _ = AuditWriter::open(tmp.path(), "sess-2").unwrap();
        let m = std::fs::metadata(tmp.path().join("sess-2.jsonl")).unwrap();
        assert_eq!(m.permissions().mode() & 0o777, 0o600);
        let dm = std::fs::metadata(tmp.path()).unwrap();
        assert_eq!(dm.permissions().mode() & 0o777, 0o700);
    }
}
