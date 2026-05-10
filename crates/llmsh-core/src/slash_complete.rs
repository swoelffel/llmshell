use reedline::{Completer, Span, Suggestion};
use std::collections::HashMap;

pub struct SlashCompleter {
    commands: Vec<&'static str>,
    subcommands: HashMap<&'static str, Vec<&'static str>>,
}

impl SlashCompleter {
    pub fn new() -> Self {
        let commands = vec![
            "help",
            "exit",
            "clear-context",
            "clear-memory",
            "clear-all",
            "compact",
            "pwd",
            "cd",
            "history",
            "init",
            "memory",
            "model",
            "provider",
        ];
        let mut subcommands = HashMap::new();
        subcommands.insert("memory", vec!["list", "forget", "add"]);
        subcommands.insert("model", vec!["list", "set"]);
        subcommands.insert("provider", vec!["list", "set"]);
        Self {
            commands,
            subcommands,
        }
    }
}

impl Default for SlashCompleter {
    fn default() -> Self {
        Self::new()
    }
}

impl Completer for SlashCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> Vec<Suggestion> {
        if !line.starts_with('/') {
            return vec![];
        }
        let head = &line[..pos.min(line.len())];
        let after_slash = &head[1..];

        let mut token_starts: Vec<usize> = Vec::new();
        let mut in_tok = false;
        for (i, c) in after_slash.char_indices() {
            if c.is_whitespace() {
                in_tok = false;
            } else if !in_tok {
                token_starts.push(i + 1);
                in_tok = true;
            }
        }
        let trailing_ws = head.len() > 1 && head.ends_with(char::is_whitespace);

        // Top-level command: 0 tokens, or 1 token without trailing space.
        if token_starts.is_empty() || (token_starts.len() == 1 && !trailing_ws) {
            let prefix_start = if token_starts.is_empty() {
                head.len()
            } else {
                token_starts[0]
            };
            let prefix = &head[prefix_start..];
            return self
                .commands
                .iter()
                .filter(|c| c.starts_with(prefix))
                .map(|c| Suggestion {
                    value: (*c).to_string(),
                    description: None,
                    style: None,
                    extra: None,
                    span: Span::new(prefix_start, head.len()),
                    append_whitespace: self.subcommands.contains_key(*c),
                })
                .collect();
        }

        // Subcommand: exactly 1 token + trailing space, or 2 tokens.
        if token_starts.len() > 2 {
            return vec![];
        }
        let cmd_start = token_starts[0];
        let cmd_end = head[cmd_start..]
            .find(char::is_whitespace)
            .map(|w| cmd_start + w)
            .unwrap_or(head.len());
        let cmd = &head[cmd_start..cmd_end];
        let Some(subs) = self.subcommands.get(cmd) else {
            return vec![];
        };
        let (prefix_start, prefix): (usize, &str) = if token_starts.len() == 2 {
            let s = token_starts[1];
            (s, &head[s..])
        } else {
            (head.len(), "")
        };
        subs.iter()
            .filter(|s| s.starts_with(prefix))
            .map(|s| Suggestion {
                value: (*s).to_string(),
                description: None,
                style: None,
                extra: None,
                span: Span::new(prefix_start, head.len()),
                append_whitespace: false,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(s: Vec<Suggestion>) -> Vec<String> {
        s.into_iter().map(|x| x.value).collect()
    }

    #[test]
    fn lone_slash_lists_all_commands() {
        let mut c = SlashCompleter::new();
        let s = names(c.complete("/", 1));
        assert!(s.contains(&"help".to_string()));
        assert!(s.contains(&"memory".to_string()));
        assert!(s.contains(&"clear-all".to_string()));
    }

    #[test]
    fn prefix_filters_top_level() {
        let mut c = SlashCompleter::new();
        assert_eq!(names(c.complete("/he", 3)), vec!["help"]);
        assert_eq!(
            names(c.complete("/clear", 6)),
            vec!["clear-context", "clear-memory", "clear-all"]
        );
    }

    #[test]
    fn memory_subcommands_after_space() {
        let mut c = SlashCompleter::new();
        assert_eq!(
            names(c.complete("/memory ", 8)),
            vec!["list", "forget", "add"]
        );
    }

    #[test]
    fn memory_subcommand_with_prefix() {
        let mut c = SlashCompleter::new();
        assert_eq!(names(c.complete("/memory f", 9)), vec!["forget"]);
    }

    #[test]
    fn model_subcommands() {
        let mut c = SlashCompleter::new();
        assert_eq!(names(c.complete("/model ", 7)), vec!["list", "set"]);
        assert_eq!(names(c.complete("/model s", 8)), vec!["set"]);
    }

    #[test]
    fn non_slash_returns_empty() {
        let mut c = SlashCompleter::new();
        assert!(c.complete("hello", 5).is_empty());
        assert!(c.complete("!ls", 3).is_empty());
    }

    #[test]
    fn deep_args_return_empty() {
        let mut c = SlashCompleter::new();
        assert!(c.complete("/memory add some text", 21).is_empty());
    }

    #[test]
    fn unknown_command_no_subs() {
        let mut c = SlashCompleter::new();
        assert!(c.complete("/help ", 6).is_empty());
    }

    #[test]
    fn span_replaces_only_after_slash() {
        let mut c = SlashCompleter::new();
        let s = c.complete("/he", 3);
        assert_eq!(s[0].value, "help");
        assert_eq!(s[0].span.start, 1);
        assert_eq!(s[0].span.end, 3);
    }
}
