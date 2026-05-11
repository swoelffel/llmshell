use crate::chain::{build_envelope, session_seed_digest};
use crate::event::AuditEvent;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

pub struct AuditWriter {
    path: PathBuf,
    file: Option<std::fs::File>,
    enabled: bool,
    seq: u64,
    last_digest: String,
}

impl AuditWriter {
    pub fn open(dir: &Path, session_id: &str) -> anyhow::Result<Self> {
        ensure_dir_secure(dir)?;
        let path = dir.join(format!("{}.jsonl", session_id));
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        set_file_perms(&path, 0o600)?;
        Ok(Self {
            path,
            file: Some(file),
            enabled: true,
            seq: 0,
            last_digest: session_seed_digest(session_id),
        })
    }

    pub fn disabled() -> Self {
        Self {
            path: PathBuf::new(),
            file: None,
            enabled: false,
            seq: 0,
            last_digest: String::new(),
        }
    }

    pub fn write(&mut self, ev: &AuditEvent) -> anyhow::Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let (envelope, this_digest) = build_envelope(ev, self.seq, &self.last_digest);
        let line = serde_json::to_string(&envelope)?;
        if let Some(f) = self.file.as_mut() {
            writeln!(f, "{}", line)?;
        }
        self.seq = self.seq.saturating_add(1);
        self.last_digest = this_digest;
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
        w.write(&AuditEvent::SessionEnded {
            ts: now_iso(),
            reason: "test".into(),
        })
        .unwrap();
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

    #[test]
    fn writes_envelope_with_seq_and_prev_digest() {
        let tmp = tempfile::tempdir().unwrap();
        let mut w = AuditWriter::open(tmp.path(), "sess-chain").unwrap();
        w.write(&AuditEvent::SessionEnded {
            ts: now_iso(),
            reason: "a".into(),
        })
        .unwrap();
        w.write(&AuditEvent::SessionEnded {
            ts: now_iso(),
            reason: "b".into(),
        })
        .unwrap();
        w.flush().unwrap();

        let s = std::fs::read_to_string(tmp.path().join("sess-chain.jsonl")).unwrap();
        let lines: Vec<&str> = s.lines().collect();
        assert_eq!(lines.len(), 2);

        let l0: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        let l1: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(l0["seq"], 0);
        assert_eq!(l1["seq"], 1);
        assert_eq!(l0["schema_version"], 6);
        assert_eq!(l1["prev_digest"], l0["digest"]);
        assert_eq!(
            l0["prev_digest"].as_str().unwrap(),
            crate::chain::session_seed_digest("sess-chain"),
        );
        assert_eq!(l0["type"], "session_ended");
        assert_eq!(l0["reason"], "a");
    }

    #[test]
    fn disabled_writer_does_not_advance_chain() {
        let mut w = AuditWriter::disabled();
        w.write(&AuditEvent::SessionEnded {
            ts: now_iso(),
            reason: "x".into(),
        })
        .unwrap();
        w.flush().unwrap();
    }
}
