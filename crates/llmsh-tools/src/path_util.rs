use std::path::{Path, PathBuf};

/// Expand a leading `~` or `~/...` against the provided home directory.
/// Other forms (including `~user`) are returned unchanged.
pub fn expand_tilde(raw: &str, home: Option<&Path>) -> PathBuf {
    if let Some(h) = home {
        if let Some(rest) = raw.strip_prefix("~/") {
            return h.join(rest);
        }
        if raw == "~" {
            return h.to_path_buf();
        }
    }
    PathBuf::from(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_tilde_slash() {
        let home = PathBuf::from("/Users/jane");
        assert_eq!(
            expand_tilde("~/Library/Caches", Some(&home)),
            PathBuf::from("/Users/jane/Library/Caches")
        );
    }

    #[test]
    fn expands_bare_tilde() {
        let home = PathBuf::from("/Users/jane");
        assert_eq!(expand_tilde("~", Some(&home)), home);
    }

    #[test]
    fn no_home_no_change() {
        assert_eq!(expand_tilde("~/foo", None), PathBuf::from("~/foo"));
    }

    #[test]
    fn unrelated_path_unchanged() {
        let home = PathBuf::from("/Users/jane");
        assert_eq!(
            expand_tilde("/etc/hosts", Some(&home)),
            PathBuf::from("/etc/hosts")
        );
        assert_eq!(
            expand_tilde("relative/path", Some(&home)),
            PathBuf::from("relative/path")
        );
    }

    #[test]
    fn tilde_user_unchanged() {
        let home = PathBuf::from("/Users/jane");
        assert_eq!(
            expand_tilde("~bob/foo", Some(&home)),
            PathBuf::from("~bob/foo")
        );
    }
}
