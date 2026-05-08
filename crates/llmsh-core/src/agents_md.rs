use std::path::{Path, PathBuf};

const BUDGET: usize = 2 * 1024; // 2 KiB hard cap
const TRUNCATION_SUFFIX: &str = "\n… (truncated)";

/// Load `~/.config/llmsh/AGENTS.md` if present. Returns None on any failure.
pub fn load_agents_md() -> Option<String> {
    let path = agents_md_path()?;
    load_agents_md_from(&path)
}

/// Variant for tests that takes an explicit path (so tests don't depend on $HOME).
pub fn load_agents_md_from(path: &Path) -> Option<String> {
    if !path.exists() {
        return None;
    }
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!("AGENTS.md unreadable at {}: {}", path.display(), e);
            return None;
        }
    };
    let s = match std::str::from_utf8(&bytes) {
        Ok(s) => s,
        Err(_) => {
            tracing::warn!(
                "AGENTS.md at {} is not valid UTF-8, ignoring",
                path.display()
            );
            return None;
        }
    };
    Some(truncate_to_budget(s))
}

fn agents_md_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "llmsh").map(|d| d.config_dir().join("AGENTS.md"))
}

fn truncate_to_budget(s: &str) -> String {
    if s.len() <= BUDGET {
        return s.to_string();
    }
    // Walk down from BUDGET to find a valid UTF-8 char boundary.
    let mut cap = BUDGET;
    while cap > 0 && !s.is_char_boundary(cap) {
        cap -= 1;
    }
    format!("{}{}", &s[..cap], TRUNCATION_SUFFIX)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn non_existent_path_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("AGENTS.md");
        assert!(load_agents_md_from(&path).is_none());
    }

    #[test]
    fn small_file_returned_unchanged() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("AGENTS.md");
        let content = "a".repeat(100);
        std::fs::write(&path, &content).unwrap();
        let result = load_agents_md_from(&path).unwrap();
        assert_eq!(result, content);
    }

    #[test]
    fn large_ascii_file_truncated_within_budget() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("AGENTS.md");
        let content = "x".repeat(5 * 1024);
        std::fs::write(&path, &content).unwrap();
        let result = load_agents_md_from(&path).unwrap();
        assert!(result.len() <= BUDGET + TRUNCATION_SUFFIX.len());
        assert!(result.ends_with(TRUNCATION_SUFFIX));
    }

    #[test]
    fn multibyte_boundary_no_panic_no_invalid_utf8() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("AGENTS.md");
        // Fill exactly up to BUDGET-1 with ASCII, then a 3-byte char so the
        // boundary splits in the middle of a code-point.
        let mut content = "a".repeat(BUDGET - 1);
        content.push('€'); // U+20AC = 3 bytes in UTF-8
        content.push_str(&"b".repeat(100)); // extra bytes after
        assert!(content.len() > BUDGET);
        std::fs::write(&path, content.as_bytes()).unwrap();
        let result = load_agents_md_from(&path).unwrap();
        // Must be valid UTF-8 and within budget + suffix
        assert!(std::str::from_utf8(result.as_bytes()).is_ok());
        assert!(result.ends_with(TRUNCATION_SUFFIX));
    }

    #[test]
    fn non_utf8_returns_none() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("AGENTS.md");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(&[0xFF, 0xFE, 0x00, 0x01]).unwrap();
        assert!(load_agents_md_from(&path).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_file_returns_none() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("AGENTS.md");
        std::fs::write(&path, "secret persona").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();

        let result = load_agents_md_from(&path);

        // Restore perms so tempdir cleanup can remove the file.
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        assert!(result.is_none());
    }
}
