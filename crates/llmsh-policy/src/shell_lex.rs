//! Quoting-aware shell lexer.
//!
//! Produces [`Lexeme`]s (words with [`Quoting`] metadata + operator variants)
//! suitable for downstream classification by `classify_shell_payload`.
//!
//! Distinguishes a real operator `|` from a quoted literal `|` inside an
//! argument (e.g. `grep -E 'a|b'`) — the original `shlex::split`-based
//! classifier could not, producing false-negative Unknown classifications.
//!
//! ## Preconditions
//!
//! Callers MUST pre-filter unsupported metacharacters (`$`, `` ` ``, `(`, `{`,
//! `\n`) upstream. As a defense in depth, this lexer:
//!
//! - treats `\n` (and other whitespace) as a token separator,
//! - treats `$`, `` ` ``, `(`, `{` as ordinary bare characters (no expansion
//!   semantics — classification of these is the upstream policy's job),
//! - rejects heredoc / here-string tokens (`<<`, `<<-`, `<<<`) as
//!   [`LexError::UnsupportedConstruct`].
//!
//! ## Out of scope
//!
//! Variable expansion, command substitution, brace/glob expansion. The lexer
//! returns the literal post-unquoting text; downstream layers decide policy.

/// Quoting provenance of a [`Token`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Quoting {
    /// All fragments were unquoted.
    Bare,
    /// Exactly one single-quoted fragment, no bare/double fragments.
    Single,
    /// Exactly one double-quoted fragment, no bare/single fragments.
    Double,
    /// A concatenation of fragments of different kinds (e.g. `--regex='a|b'`).
    Mixed,
}

/// A lexed word with its post-unquoting `value` and the originating
/// [`Quoting`] flavour.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Token {
    pub value: String,
    pub quoting: Quoting,
}

/// Shell operators recognised by the lexer.
///
/// Note that file-descriptor duplications such as `2>&1` are emitted as one
/// atomic [`Operator::FdDup`] with the literal text preserved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Operator {
    /// `|`
    Pipe,
    /// `&&`
    AndIf,
    /// `||`
    OrIf,
    /// `&` (not followed by another `&`)
    Background,
    /// `;`
    Semicolon,
    /// `>`
    RedirOut,
    /// `>>`
    RedirAppend,
    /// `1>` or `2>`
    RedirOutN(u8),
    /// `1>>` or `2>>`
    RedirAppendN(u8),
    /// `&>`
    RedirAll,
    /// `&>>`
    RedirAllAppend,
    /// `<`
    RedirIn,
    /// FD duplication token: `2>&1`, `1>&2`, `>&1`, `>&2` (literal text).
    FdDup(String),
}

/// A lexed unit: a word or an operator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Lexeme {
    Word(Token),
    Op(Operator),
}

/// Lexer errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LexError {
    /// A single or double quote was never closed.
    UnterminatedQuote,
    /// Input ended with a trailing backslash in bare context.
    DanglingEscape,
    /// Heredoc / here-string or other syntactic construct unsupported here.
    UnsupportedConstruct,
}

/// Lex `payload` into a sequence of [`Lexeme`]s.
///
/// See module-level docs for preconditions and semantics.
pub(crate) fn lex(payload: &str) -> Result<Vec<Lexeme>, LexError> {
    let bytes = payload.as_bytes();
    let mut out: Vec<Lexeme> = Vec::new();
    let mut i = 0;
    let len = bytes.len();

    // Accumulator for the current word (which may be a concatenation of
    // bare/single/double fragments).
    let mut buf = String::new();
    let mut had_bare = false;
    let mut had_single = false;
    let mut had_double = false;
    let mut in_word = false;

    // Helper closure: flush current word to `out`.
    let flush = |out: &mut Vec<Lexeme>,
                 buf: &mut String,
                 in_word: &mut bool,
                 had_bare: &mut bool,
                 had_single: &mut bool,
                 had_double: &mut bool| {
        if *in_word {
            let kinds = (*had_bare as u8) + (*had_single as u8) + (*had_double as u8);
            let quoting = if kinds <= 1 {
                if *had_single {
                    Quoting::Single
                } else if *had_double {
                    Quoting::Double
                } else {
                    Quoting::Bare
                }
            } else {
                Quoting::Mixed
            };
            out.push(Lexeme::Word(Token {
                value: std::mem::take(buf),
                quoting,
            }));
            *in_word = false;
            *had_bare = false;
            *had_single = false;
            *had_double = false;
        }
    };

    while i < len {
        let c = bytes[i];

        // Whitespace (including \n as defense in depth): word separator.
        if c == b' ' || c == b'\t' || c == b'\n' || c == b'\r' {
            flush(
                &mut out,
                &mut buf,
                &mut in_word,
                &mut had_bare,
                &mut had_single,
                &mut had_double,
            );
            i += 1;
            continue;
        }

        // Single quote: literal until next '.
        if c == b'\'' {
            i += 1;
            let start = i;
            while i < len && bytes[i] != b'\'' {
                i += 1;
            }
            if i >= len {
                return Err(LexError::UnterminatedQuote);
            }
            // bytes[start..i] is the literal content.
            // Safe: payload is &str, single quote is ASCII and never splits
            // multi-byte UTF-8 codepoints.
            buf.push_str(&payload[start..i]);
            i += 1; // consume closing '
            in_word = true;
            had_single = true;
            continue;
        }

        // Double quote: literal except backslash escapes for " and \.
        if c == b'"' {
            i += 1;
            while i < len && bytes[i] != b'"' {
                if bytes[i] == b'\\' && i + 1 < len {
                    let nxt = bytes[i + 1];
                    if nxt == b'"' || nxt == b'\\' {
                        buf.push(nxt as char);
                        i += 2;
                        continue;
                    }
                    // Other \X: bash preserves both characters.
                    buf.push('\\');
                    // The next byte may start a multi-byte UTF-8 sequence; we
                    // need to copy a full char, not just one byte.
                    let rest = &payload[i + 1..];
                    if let Some(ch) = rest.chars().next() {
                        buf.push(ch);
                        i += 1 + ch.len_utf8();
                    } else {
                        return Err(LexError::UnterminatedQuote);
                    }
                    continue;
                }
                // Copy one full UTF-8 char.
                let rest = &payload[i..];
                if let Some(ch) = rest.chars().next() {
                    buf.push(ch);
                    i += ch.len_utf8();
                } else {
                    return Err(LexError::UnterminatedQuote);
                }
            }
            if i >= len {
                return Err(LexError::UnterminatedQuote);
            }
            i += 1; // consume closing "
            in_word = true;
            had_double = true;
            continue;
        }

        // Bare escape: \c consumes the next char literally.
        if c == b'\\' {
            if i + 1 >= len {
                return Err(LexError::DanglingEscape);
            }
            let rest = &payload[i + 1..];
            if let Some(ch) = rest.chars().next() {
                buf.push(ch);
                i += 1 + ch.len_utf8();
                in_word = true;
                had_bare = true;
                continue;
            } else {
                return Err(LexError::DanglingEscape);
            }
        }

        // Operator detection (only in bare context). Match longest-first.

        // Heredoc / here-string: rejected.
        if c == b'<' && i + 1 < len && bytes[i + 1] == b'<' {
            return Err(LexError::UnsupportedConstruct);
        }

        // FD duplications: 2>&1, 1>&2, >&1, >&2 (only these four shapes).
        if let Some((tok, advance)) = try_fd_dup(bytes, i) {
            flush(
                &mut out,
                &mut buf,
                &mut in_word,
                &mut had_bare,
                &mut had_single,
                &mut had_double,
            );
            out.push(Lexeme::Op(Operator::FdDup(tok)));
            i += advance;
            continue;
        }

        // Numbered redirections: 1>>, 2>>, 1>, 2>.
        if (c == b'1' || c == b'2') && i + 1 < len && bytes[i + 1] == b'>' {
            let fd = c - b'0';
            if i + 2 < len && bytes[i + 2] == b'>' {
                flush(
                    &mut out,
                    &mut buf,
                    &mut in_word,
                    &mut had_bare,
                    &mut had_single,
                    &mut had_double,
                );
                out.push(Lexeme::Op(Operator::RedirAppendN(fd)));
                i += 3;
                continue;
            }
            flush(
                &mut out,
                &mut buf,
                &mut in_word,
                &mut had_bare,
                &mut had_single,
                &mut had_double,
            );
            out.push(Lexeme::Op(Operator::RedirOutN(fd)));
            i += 2;
            continue;
        }

        // `&&`, `&>>`, `&>`, `&`.
        if c == b'&' {
            if i + 1 < len && bytes[i + 1] == b'&' {
                flush(
                    &mut out,
                    &mut buf,
                    &mut in_word,
                    &mut had_bare,
                    &mut had_single,
                    &mut had_double,
                );
                out.push(Lexeme::Op(Operator::AndIf));
                i += 2;
                continue;
            }
            if i + 2 < len && bytes[i + 1] == b'>' && bytes[i + 2] == b'>' {
                flush(
                    &mut out,
                    &mut buf,
                    &mut in_word,
                    &mut had_bare,
                    &mut had_single,
                    &mut had_double,
                );
                out.push(Lexeme::Op(Operator::RedirAllAppend));
                i += 3;
                continue;
            }
            if i + 1 < len && bytes[i + 1] == b'>' {
                flush(
                    &mut out,
                    &mut buf,
                    &mut in_word,
                    &mut had_bare,
                    &mut had_single,
                    &mut had_double,
                );
                out.push(Lexeme::Op(Operator::RedirAll));
                i += 2;
                continue;
            }
            flush(
                &mut out,
                &mut buf,
                &mut in_word,
                &mut had_bare,
                &mut had_single,
                &mut had_double,
            );
            out.push(Lexeme::Op(Operator::Background));
            i += 1;
            continue;
        }

        // `||`, `|`.
        if c == b'|' {
            if i + 1 < len && bytes[i + 1] == b'|' {
                flush(
                    &mut out,
                    &mut buf,
                    &mut in_word,
                    &mut had_bare,
                    &mut had_single,
                    &mut had_double,
                );
                out.push(Lexeme::Op(Operator::OrIf));
                i += 2;
                continue;
            }
            flush(
                &mut out,
                &mut buf,
                &mut in_word,
                &mut had_bare,
                &mut had_single,
                &mut had_double,
            );
            out.push(Lexeme::Op(Operator::Pipe));
            i += 1;
            continue;
        }

        // `;`.
        if c == b';' {
            flush(
                &mut out,
                &mut buf,
                &mut in_word,
                &mut had_bare,
                &mut had_single,
                &mut had_double,
            );
            out.push(Lexeme::Op(Operator::Semicolon));
            i += 1;
            continue;
        }

        // `>>`, `>`.
        if c == b'>' {
            if i + 1 < len && bytes[i + 1] == b'>' {
                flush(
                    &mut out,
                    &mut buf,
                    &mut in_word,
                    &mut had_bare,
                    &mut had_single,
                    &mut had_double,
                );
                out.push(Lexeme::Op(Operator::RedirAppend));
                i += 2;
                continue;
            }
            flush(
                &mut out,
                &mut buf,
                &mut in_word,
                &mut had_bare,
                &mut had_single,
                &mut had_double,
            );
            out.push(Lexeme::Op(Operator::RedirOut));
            i += 1;
            continue;
        }

        // `<` (heredoc already handled above).
        if c == b'<' {
            flush(
                &mut out,
                &mut buf,
                &mut in_word,
                &mut had_bare,
                &mut had_single,
                &mut had_double,
            );
            out.push(Lexeme::Op(Operator::RedirIn));
            i += 1;
            continue;
        }

        // Plain bare character — copy one full UTF-8 char.
        let rest = &payload[i..];
        if let Some(ch) = rest.chars().next() {
            buf.push(ch);
            i += ch.len_utf8();
            in_word = true;
            had_bare = true;
        } else {
            // Unreachable: i < len and payload is valid UTF-8.
            break;
        }
    }

    flush(
        &mut out,
        &mut buf,
        &mut in_word,
        &mut had_bare,
        &mut had_single,
        &mut had_double,
    );

    Ok(out)
}

/// Try to match a file-descriptor duplication token starting at `i`.
/// Returns `(literal_text, bytes_consumed)`.
fn try_fd_dup(bytes: &[u8], i: usize) -> Option<(String, usize)> {
    // Patterns: "2>&1", "1>&2", ">&1", ">&2".
    let len = bytes.len();
    // 4-byte patterns.
    if i + 4 <= len {
        let s = &bytes[i..i + 4];
        if s == b"2>&1" || s == b"1>&2" {
            // Safe: ASCII.
            return Some((std::str::from_utf8(s).ok()?.to_string(), 4));
        }
    }
    // 3-byte patterns.
    if i + 3 <= len {
        let s = &bytes[i..i + 3];
        if s == b">&1" || s == b">&2" {
            return Some((std::str::from_utf8(s).ok()?.to_string(), 3));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn w(s: &str, q: Quoting) -> Lexeme {
        Lexeme::Word(Token {
            value: s.to_string(),
            quoting: q,
        })
    }

    // 1
    #[test]
    fn test_single_bare_word() {
        assert_eq!(lex("ls").unwrap(), vec![w("ls", Quoting::Bare)]);
    }

    // 2
    #[test]
    fn test_two_bare_words() {
        assert_eq!(
            lex("ls -la").unwrap(),
            vec![w("ls", Quoting::Bare), w("-la", Quoting::Bare)]
        );
    }

    // 3
    #[test]
    fn test_single_quoted() {
        assert_eq!(
            lex("'hello world'").unwrap(),
            vec![w("hello world", Quoting::Single)]
        );
    }

    // 4
    #[test]
    fn test_double_quoted() {
        assert_eq!(
            lex("\"hello world\"").unwrap(),
            vec![w("hello world", Quoting::Double)]
        );
    }

    // 5
    #[test]
    fn test_mixed_quoting() {
        assert_eq!(
            lex("--regex='a|b'").unwrap(),
            vec![w("--regex=a|b", Quoting::Mixed)]
        );
    }

    // 6
    #[test]
    fn test_pipe_spaced() {
        assert_eq!(
            lex("a | b").unwrap(),
            vec![
                w("a", Quoting::Bare),
                Lexeme::Op(Operator::Pipe),
                w("b", Quoting::Bare),
            ]
        );
    }

    // 7 — the original bug
    #[test]
    fn test_pipe_glued() {
        assert_eq!(
            lex("a|b").unwrap(),
            vec![
                w("a", Quoting::Bare),
                Lexeme::Op(Operator::Pipe),
                w("b", Quoting::Bare),
            ]
        );
    }

    // 8 — the other side of the bug
    #[test]
    fn test_pipe_quoted_is_literal() {
        assert_eq!(lex("'a|b'").unwrap(), vec![w("a|b", Quoting::Single)]);
    }

    // 9
    #[test]
    fn test_and_if() {
        assert_eq!(
            lex("a && b").unwrap(),
            vec![
                w("a", Quoting::Bare),
                Lexeme::Op(Operator::AndIf),
                w("b", Quoting::Bare),
            ]
        );
    }

    // 10
    #[test]
    fn test_or_if() {
        assert_eq!(
            lex("a || b").unwrap(),
            vec![
                w("a", Quoting::Bare),
                Lexeme::Op(Operator::OrIf),
                w("b", Quoting::Bare),
            ]
        );
    }

    // 11
    #[test]
    fn test_and_if_glued() {
        assert_eq!(
            lex("a&&b").unwrap(),
            vec![
                w("a", Quoting::Bare),
                Lexeme::Op(Operator::AndIf),
                w("b", Quoting::Bare),
            ]
        );
    }

    // 12
    #[test]
    fn test_background() {
        assert_eq!(
            lex("a &").unwrap(),
            vec![w("a", Quoting::Bare), Lexeme::Op(Operator::Background)]
        );
    }

    // 13
    #[test]
    fn test_background_not_consumed_by_andif() {
        // `a && b` must yield AndIf, not Background+Background or such.
        let v = lex("a && b").unwrap();
        assert!(matches!(v[1], Lexeme::Op(Operator::AndIf)));
    }

    // 14
    #[test]
    fn test_semicolon() {
        assert_eq!(
            lex("a ; b").unwrap(),
            vec![
                w("a", Quoting::Bare),
                Lexeme::Op(Operator::Semicolon),
                w("b", Quoting::Bare),
            ]
        );
    }

    // 15
    #[test]
    fn test_redir_out() {
        assert_eq!(
            lex("cmd > /dev/null").unwrap(),
            vec![
                w("cmd", Quoting::Bare),
                Lexeme::Op(Operator::RedirOut),
                w("/dev/null", Quoting::Bare),
            ]
        );
    }

    // 16
    #[test]
    fn test_redir_append() {
        assert_eq!(
            lex("cmd >> /tmp/x").unwrap(),
            vec![
                w("cmd", Quoting::Bare),
                Lexeme::Op(Operator::RedirAppend),
                w("/tmp/x", Quoting::Bare),
            ]
        );
    }

    // 17
    #[test]
    fn test_redir_in() {
        assert_eq!(
            lex("cmd < input").unwrap(),
            vec![
                w("cmd", Quoting::Bare),
                Lexeme::Op(Operator::RedirIn),
                w("input", Quoting::Bare),
            ]
        );
    }

    // 18
    #[test]
    fn test_numbered_redirs() {
        assert_eq!(
            lex("cmd 1> a 2> b 1>> c 2>> d").unwrap(),
            vec![
                w("cmd", Quoting::Bare),
                Lexeme::Op(Operator::RedirOutN(1)),
                w("a", Quoting::Bare),
                Lexeme::Op(Operator::RedirOutN(2)),
                w("b", Quoting::Bare),
                Lexeme::Op(Operator::RedirAppendN(1)),
                w("c", Quoting::Bare),
                Lexeme::Op(Operator::RedirAppendN(2)),
                w("d", Quoting::Bare),
            ]
        );
    }

    // 19
    #[test]
    fn test_redir_all() {
        assert_eq!(
            lex("cmd &> log").unwrap(),
            vec![
                w("cmd", Quoting::Bare),
                Lexeme::Op(Operator::RedirAll),
                w("log", Quoting::Bare),
            ]
        );
        assert_eq!(
            lex("cmd &>> log").unwrap(),
            vec![
                w("cmd", Quoting::Bare),
                Lexeme::Op(Operator::RedirAllAppend),
                w("log", Quoting::Bare),
            ]
        );
    }

    // 20
    #[test]
    fn test_fd_dup_2_to_1() {
        assert_eq!(
            lex("cmd 2>&1").unwrap(),
            vec![
                w("cmd", Quoting::Bare),
                Lexeme::Op(Operator::FdDup("2>&1".to_string())),
            ]
        );
    }

    // 21
    #[test]
    fn test_fd_dup_to_stderr() {
        assert_eq!(
            lex("cmd >&2").unwrap(),
            vec![
                w("cmd", Quoting::Bare),
                Lexeme::Op(Operator::FdDup(">&2".to_string())),
            ]
        );
    }

    // 22
    #[test]
    fn test_unterminated_single_quote() {
        assert_eq!(lex("'abc"), Err(LexError::UnterminatedQuote));
    }

    // 23
    #[test]
    fn test_unterminated_double_quote() {
        assert_eq!(lex("\"abc"), Err(LexError::UnterminatedQuote));
    }

    // 24
    #[test]
    fn test_bare_escape_pipe() {
        assert_eq!(lex(r"a\|b").unwrap(), vec![w("a|b", Quoting::Bare)]);
    }

    // 25
    #[test]
    fn test_dangling_escape() {
        assert_eq!(lex("abc\\"), Err(LexError::DanglingEscape));
    }

    // 26
    #[test]
    fn test_escape_in_double_quote_quote() {
        assert_eq!(lex(r#""a\"b""#).unwrap(), vec![w("a\"b", Quoting::Double)]);
    }

    // 27
    #[test]
    fn test_escape_in_double_quote_backslash() {
        assert_eq!(lex(r#""a\\b""#).unwrap(), vec![w(r"a\b", Quoting::Double)]);
    }

    // 28
    #[test]
    fn test_backslash_literal_in_single_quote() {
        assert_eq!(lex(r"'a\b'").unwrap(), vec![w(r"a\b", Quoting::Single)]);
    }

    // 29
    #[test]
    fn test_lone_orif() {
        assert_eq!(lex("||").unwrap(), vec![Lexeme::Op(Operator::OrIf)]);
    }

    // 30 — the target real-world case
    #[test]
    fn test_real_world_df_grep() {
        assert_eq!(
            lex(r"df -h | grep -E '^/dev|^Filesystem'").unwrap(),
            vec![
                w("df", Quoting::Bare),
                w("-h", Quoting::Bare),
                Lexeme::Op(Operator::Pipe),
                w("grep", Quoting::Bare),
                w("-E", Quoting::Bare),
                w("^/dev|^Filesystem", Quoting::Single),
            ]
        );
    }

    // 31
    #[test]
    fn test_heredoc_rejected() {
        assert_eq!(lex("cat <<EOF"), Err(LexError::UnsupportedConstruct));
    }

    // 32
    #[test]
    fn test_heredoc_strip_rejected() {
        assert_eq!(lex("cat <<-EOF"), Err(LexError::UnsupportedConstruct));
    }

    // 33
    #[test]
    fn test_herestring_rejected() {
        assert_eq!(lex("cat <<<x"), Err(LexError::UnsupportedConstruct));
    }

    // 34
    #[test]
    fn test_pipe_with_fd_dup() {
        assert_eq!(
            lex("cmd 2>&1 | grep x").unwrap(),
            vec![
                w("cmd", Quoting::Bare),
                Lexeme::Op(Operator::FdDup("2>&1".to_string())),
                Lexeme::Op(Operator::Pipe),
                w("grep", Quoting::Bare),
                w("x", Quoting::Bare),
            ]
        );
    }

    // 35
    #[test]
    fn test_glob_chars_passthrough() {
        assert_eq!(
            lex("ls *.rs").unwrap(),
            vec![w("ls", Quoting::Bare), w("*.rs", Quoting::Bare)]
        );
    }

    // 36
    #[test]
    fn test_newline_as_separator() {
        assert_eq!(
            lex("a\nb").unwrap(),
            vec![w("a", Quoting::Bare), w("b", Quoting::Bare)]
        );
    }

    // --- extra robustness ---

    #[test]
    fn test_empty_input() {
        assert_eq!(lex("").unwrap(), Vec::<Lexeme>::new());
    }

    #[test]
    fn test_whitespace_only() {
        assert_eq!(lex("   \t\n  ").unwrap(), Vec::<Lexeme>::new());
    }

    #[test]
    fn test_mixed_three_fragments() {
        // bare + single + double => Mixed
        assert_eq!(lex(r#"a'b'"c""#).unwrap(), vec![w("abc", Quoting::Mixed)]);
    }

    #[test]
    fn test_unknown_fd_treated_as_bare() {
        // `3>` — '3' is not a recognised FD; the lexer treats '3' as a bare
        // character, then '>' splits the word and emits RedirOut.
        assert_eq!(
            lex("cmd 3> x").unwrap(),
            vec![
                w("cmd", Quoting::Bare),
                w("3", Quoting::Bare),
                Lexeme::Op(Operator::RedirOut),
                w("x", Quoting::Bare),
            ]
        );
    }

    #[test]
    fn test_double_quote_preserves_other_escape() {
        // `"\n"` — bash preserves both characters since \n is not \" or \\.
        assert_eq!(lex(r#""\n""#).unwrap(), vec![w(r"\n", Quoting::Double)]);
    }
}
