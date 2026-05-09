//! Shared mutable PWD with helpers.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

pub type SharedCwd = Arc<RwLock<PathBuf>>;

pub fn new_shared(initial: PathBuf) -> SharedCwd {
    Arc::new(RwLock::new(initial))
}

pub fn snapshot(cwd: &SharedCwd) -> PathBuf {
    cwd.read().unwrap().clone()
}

#[derive(Debug)]
pub enum ChdirError {
    NotFound,
    NotADirectory,
    Io(std::io::Error),
}

impl std::fmt::Display for ChdirError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "no such file or directory"),
            Self::NotADirectory => write!(f, "not a directory"),
            Self::Io(e) => write!(f, "{}", e),
        }
    }
}

impl std::error::Error for ChdirError {}

/// Try to change the shared cwd to `target`. On success: canonicalizes,
/// validates is-a-directory, calls `std::env::set_current_dir`, and updates
/// the shared lock. Returns the new canonical path.
pub fn try_chdir(cwd: &SharedCwd, target: &Path) -> Result<PathBuf, ChdirError> {
    let canonical = std::fs::canonicalize(target).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            ChdirError::NotFound
        } else {
            ChdirError::Io(e)
        }
    })?;
    let meta = std::fs::metadata(&canonical).map_err(ChdirError::Io)?;
    if !meta.is_dir() {
        return Err(ChdirError::NotADirectory);
    }
    std::env::set_current_dir(&canonical).map_err(ChdirError::Io)?;
    *cwd.write().unwrap() = canonical.clone();
    Ok(canonical)
}

/// Resolve a `cd` argument into an absolute path. Handles:
/// - empty / None → $HOME
/// - "-" → $OLDPWD
/// - "~" or "~/foo" → $HOME-relative
/// - relative → joined to current snapshot
pub fn resolve_cd_target(
    arg: Option<&str>,
    current: &Path,
    home: Option<&Path>,
    oldpwd: Option<&Path>,
) -> Result<PathBuf, ChdirError> {
    let raw = arg.map(str::trim).unwrap_or("");
    if raw.is_empty() {
        return home.map(Path::to_path_buf).ok_or(ChdirError::NotFound);
    }
    if raw == "-" {
        return oldpwd.map(Path::to_path_buf).ok_or(ChdirError::NotFound);
    }
    if let Some(rest) = raw.strip_prefix('~') {
        let home = home.ok_or(ChdirError::NotFound)?;
        let rest = rest.strip_prefix('/').unwrap_or(rest);
        return Ok(if rest.is_empty() {
            home.to_path_buf()
        } else {
            home.join(rest)
        });
    }
    let p = PathBuf::from(raw);
    if p.is_absolute() {
        Ok(p)
    } else {
        Ok(current.join(p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn try_chdir_to_existing_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let sub = tmp.path().join("sub");
        fs::create_dir(&sub).unwrap();
        let saved = std::env::current_dir().ok();
        let cwd = new_shared(tmp.path().to_path_buf());
        let result = try_chdir(&cwd, &sub).unwrap();
        let canonical_sub = std::fs::canonicalize(&sub).unwrap();
        assert_eq!(snapshot(&cwd), canonical_sub);
        assert_eq!(result, canonical_sub);
        if let Some(p) = saved {
            let _ = std::env::set_current_dir(p);
        }
    }

    #[test]
    fn try_chdir_nonexistent_returns_not_found() {
        let cwd = new_shared(PathBuf::from("/"));
        let err = try_chdir(&cwd, &PathBuf::from("/zzz_nope_zzz_xyz_does_not_exist")).unwrap_err();
        assert!(matches!(err, ChdirError::NotFound));
    }

    #[test]
    fn try_chdir_to_file_returns_not_a_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("f");
        fs::write(&file, b"x").unwrap();
        let cwd = new_shared(tmp.path().to_path_buf());
        let err = try_chdir(&cwd, &file).unwrap_err();
        assert!(matches!(err, ChdirError::NotADirectory));
    }

    #[test]
    fn resolve_empty_returns_home() {
        let home = PathBuf::from("/home/u");
        let r = resolve_cd_target(None, &PathBuf::from("/tmp"), Some(&home), None).unwrap();
        assert_eq!(r, home);
    }

    #[test]
    fn resolve_dash_returns_oldpwd() {
        let old = PathBuf::from("/old");
        let r = resolve_cd_target(
            Some("-"),
            &PathBuf::from("/cur"),
            Some(&PathBuf::from("/h")),
            Some(&old),
        )
        .unwrap();
        assert_eq!(r, old);
    }

    #[test]
    fn resolve_tilde_expands() {
        let home = PathBuf::from("/home/u");
        assert_eq!(
            resolve_cd_target(Some("~"), &PathBuf::from("/x"), Some(&home), None).unwrap(),
            home
        );
        assert_eq!(
            resolve_cd_target(Some("~/foo"), &PathBuf::from("/x"), Some(&home), None).unwrap(),
            home.join("foo")
        );
    }

    #[test]
    fn resolve_relative_joins_current() {
        let r = resolve_cd_target(Some("foo/bar"), &PathBuf::from("/base"), None, None).unwrap();
        assert_eq!(r, PathBuf::from("/base/foo/bar"));
    }

    #[test]
    fn resolve_absolute_used_as_is() {
        let r = resolve_cd_target(Some("/etc"), &PathBuf::from("/cur"), None, None).unwrap();
        assert_eq!(r, PathBuf::from("/etc"));
    }
}
