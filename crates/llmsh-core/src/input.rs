#[derive(Debug, PartialEq)]
pub enum InputKind {
    Empty,
    Meta(String, Vec<String>),
    RawShell(String),
    Natural(String),
}

pub fn classify(line: &str) -> InputKind {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return InputKind::Empty;
    }
    if let Some(rest) = trimmed.strip_prefix('!') {
        return InputKind::RawShell(rest.to_string());
    }
    if let Some(rest) = trimmed.strip_prefix('/') {
        let mut parts = rest.split_whitespace();
        let cmd = parts.next().unwrap_or("").to_string();
        let args: Vec<String> = parts.map(String::from).collect();
        return InputKind::Meta(cmd, args);
    }
    InputKind::Natural(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty() {
        assert_eq!(classify("   "), InputKind::Empty);
    }
    #[test]
    fn meta() {
        assert_eq!(
            classify("/cd ../foo"),
            InputKind::Meta("cd".into(), vec!["../foo".into()])
        );
    }
    #[test]
    fn raw_preserves_internals() {
        assert_eq!(
            classify("! ls -la 'a b'"),
            InputKind::RawShell(" ls -la 'a b'".into())
        );
    }
    #[test]
    fn natural() {
        assert_eq!(
            classify("liste les fichiers"),
            InputKind::Natural("liste les fichiers".into())
        );
    }
}
