use std::path::{Path, PathBuf};

pub fn canonicalize_lenient(p: &Path, cwd: &Path) -> PathBuf {
    let raw = if p.is_absolute() { p.to_path_buf() } else { cwd.join(p) };
    std::fs::canonicalize(&raw).unwrap_or_else(|_| normalize(&raw))
}

fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            std::path::Component::ParentDir => { out.pop(); }
            std::path::Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    out
}

pub fn is_inside(child: &Path, parents: &[PathBuf]) -> bool {
    parents.iter().any(|p| {
        let canon_parent = std::fs::canonicalize(p).unwrap_or_else(|_| p.clone());
        let canon_child = std::fs::canonicalize(child).unwrap_or_else(|_| child.to_path_buf());
        canon_child.starts_with(canon_parent)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parent_dir_normalized() {
        let p = Path::new("/tmp/a/../b");
        assert_eq!(normalize(p), PathBuf::from("/tmp/b"));
    }

    #[test]
    fn relative_resolved_against_cwd() {
        let p = canonicalize_lenient(Path::new("./x"), Path::new("/tmp"));
        assert!(p.ends_with("x"));
    }
}
