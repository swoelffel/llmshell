use std::path::Path;

pub fn matches_sensitive(path: &Path, patterns: &[String], home: Option<&Path>) -> bool {
    let s = path.to_string_lossy();
    for raw in patterns {
        let pat = if let Some(rest) = raw.strip_prefix("~/") {
            match home {
                Some(h) => format!("{}/{}", h.display(), rest),
                None => raw.clone(),
            }
        } else { raw.clone() };
        if glob_match(&pat, &s) { return true; }
    }
    false
}

fn glob_match(pat: &str, s: &str) -> bool {
    let pat_b = pat.as_bytes();
    let s_b = s.as_bytes();
    glob_inner(pat_b, 0, s_b, 0)
}

fn glob_inner(p: &[u8], mut pi: usize, s: &[u8], mut si: usize) -> bool {
    while pi < p.len() {
        if p[pi] == b'*' && pi + 1 < p.len() && p[pi + 1] == b'*' {
            let next = pi + 2;
            if next == p.len() { return true; }
            let after = if p[next] == b'/' { next + 1 } else { next };
            for k in si..=s.len() {
                if glob_inner(p, after, s, k) { return true; }
            }
            return false;
        }
        if p[pi] == b'*' {
            let next = pi + 1;
            if next == p.len() { return !s[si..].contains(&b'/'); }
            for k in si..=s.len() {
                if k > si && s[k-1] == b'/' { return false; }
                if glob_inner(p, next, s, k) { return true; }
            }
            return false;
        }
        if si >= s.len() { return false; }
        if p[pi] != s[si] { return false; }
        pi += 1; si += 1;
    }
    si == s.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn matches_ssh_key() {
        let pats = vec!["~/.ssh/**".to_string(), "**/id_rsa".to_string()];
        let home = PathBuf::from("/home/u");
        assert!(matches_sensitive(Path::new("/home/u/.ssh/id_ed25519"), &pats, Some(&home)));
        assert!(matches_sensitive(Path::new("/tmp/backup/id_rsa"), &pats, Some(&home)));
        assert!(!matches_sensitive(Path::new("/tmp/normal.txt"), &pats, Some(&home)));
    }

    #[test]
    fn matches_dotenv() {
        let pats = vec![".env*".to_string()];
        assert!(matches_sensitive(Path::new(".env"), &pats, None));
        assert!(matches_sensitive(Path::new(".env.local"), &pats, None));
        assert!(!matches_sensitive(Path::new("envfile"), &pats, None));
    }
}
