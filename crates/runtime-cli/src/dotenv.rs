//! A small, deterministic `.env` parser for `esrun --env-file` (DECISIONS D30).
//!
//! `.env` has no specification and implementations disagree; this one fixes a
//! predictable, documented dialect suited to a production server runtime:
//!
//! - `KEY=value` per line; blank lines and `#` comment lines are ignored.
//! - An optional `export ` prefix is stripped (shell-`source` compatibility).
//! - Keys must match `[A-Za-z_][A-Za-z0-9_]*`; whitespace around `=` is trimmed.
//! - **Double-quoted** values process `\n \r \t \\ \"` escapes and may span
//!   multiple lines; **single-quoted** values are literal and may span lines.
//! - **Unquoted** values are trimmed; an inline `#` preceded by whitespace
//!   starts a comment.
//! - **No variable expansion** (`${VAR}`/`$VAR` are literal) — deliberate, for
//!   predictability (D30).
//! - A leading UTF-8 BOM is stripped; both `\n` and `\r\n` line endings work.
//! - Within a file, a later assignment to the same key wins.
//!
//! The parser never echoes a value in an error (values are secrets); errors
//! carry the file label and 1-based line number only.

use std::path::Path;

/// Reads and parses an `.env` file, returning its entries in file order (later
/// duplicates kept — the caller folds them, last-wins). The `label` (the path
/// as the user wrote it) is used in error messages; values are never included.
pub fn load(path: &Path) -> Result<Vec<(String, String)>, String> {
    let label = path.display();
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("--env-file {label}: cannot read file: {e}"))?;
    parse(&raw).map_err(|e| format!("--env-file {label}:{}: {}", e.line, e.message))
}

/// A parse error: a 1-based line number and a message that never contains a
/// variable's value.
#[derive(Debug)]
struct ParseError {
    line: usize,
    message: String,
}

fn err(line: usize, message: impl Into<String>) -> ParseError {
    ParseError {
        line,
        message: message.into(),
    }
}

fn parse(input: &str) -> Result<Vec<(String, String)>, ParseError> {
    // Strip a leading UTF-8 BOM, then normalize away `\r` so `\r\n` files parse
    // like `\n` ones (a stray `\r` inside a quoted value is also dropped, which
    // matches what every shell/dotenv reader does on Windows files).
    let input = input.strip_prefix('\u{feff}').unwrap_or(input);
    let lines: Vec<&str> = input
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
        .collect();

    let mut out = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line_no = i + 1;
        let line = lines[i];
        i += 1;

        let trimmed = line.trim_start();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Optional `export ` prefix (shell-source compatibility).
        let body = trimmed
            .strip_prefix("export ")
            .map(str::trim_start)
            .unwrap_or(trimmed);

        let eq = body
            .find('=')
            .ok_or_else(|| err(line_no, "expected KEY=value"))?;
        let key = body[..eq].trim_end();
        validate_key(key, line_no)?;
        let rest = &body[eq + 1..];

        let value = match rest.trim_start().chars().next() {
            Some('"') => parse_quoted(&lines, &mut i, rest.trim_start(), line_no, '"', true)?,
            Some('\'') => parse_quoted(&lines, &mut i, rest.trim_start(), line_no, '\'', false)?,
            _ => parse_unquoted(rest),
        };
        out.push((key.to_string(), value));
    }
    Ok(out)
}

fn validate_key(key: &str, line_no: usize) -> Result<(), ParseError> {
    let mut chars = key.chars();
    let ok = matches!(chars.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    if ok {
        Ok(())
    } else if key.is_empty() {
        Err(err(line_no, "empty key before '='"))
    } else {
        Err(err(
            line_no,
            "invalid key (expected letters, digits, and '_', not starting with a digit)",
        ))
    }
}

/// Parses a quoted value beginning at `first` (the value text with leading
/// whitespace already removed, starting with the quote char). For double quotes
/// (`escapes = true`), `\n \r \t \\ \"` are decoded; single quotes are literal.
/// A value may span lines: if the closing quote is not on the current line,
/// subsequent lines (joined with `\n`) are consumed via `*i`.
fn parse_quoted(
    lines: &[&str],
    i: &mut usize,
    first: &str,
    start_line: usize,
    quote: char,
    escapes: bool,
) -> Result<String, ParseError> {
    let mut value = String::new();
    // `segment` is the text after the opening quote on the first line; further
    // lines are appended (prefixed with the `\n` that split removed).
    let mut segment = &first[quote.len_utf8()..];
    loop {
        let mut chars = segment.char_indices();
        while let Some((idx, c)) = chars.next() {
            if escapes && c == '\\' {
                match chars.next() {
                    Some((_, 'n')) => value.push('\n'),
                    Some((_, 'r')) => value.push('\r'),
                    Some((_, 't')) => value.push('\t'),
                    Some((_, '\\')) => value.push('\\'),
                    Some((_, '"')) => value.push('"'),
                    // Unknown escape: keep the backslash and the char verbatim.
                    Some((j, other)) => {
                        value.push('\\');
                        value.push(other);
                        let _ = (idx, j);
                    }
                    None => value.push('\\'),
                }
            } else if c == quote {
                return Ok(value);
            } else {
                value.push(c);
            }
        }
        // Closing quote not found on this line — continue on the next.
        if *i >= lines.len() {
            return Err(err(start_line, "unterminated quoted value"));
        }
        value.push('\n');
        segment = lines[*i];
        *i += 1;
    }
}

/// Parses an unquoted value: trims surrounding whitespace and strips an inline
/// comment introduced by ` #` (a `#` preceded by whitespace). A value that is
/// only a comment (or empty) yields an empty string.
fn parse_unquoted(rest: &str) -> String {
    let s = rest.trim_start();
    if s.is_empty() || s.starts_with('#') {
        return String::new();
    }
    // Find a `#` preceded by whitespace; everything from there is a comment.
    let mut cut = s.len();
    let bytes = s.as_bytes();
    for idx in 1..bytes.len() {
        if bytes[idx] == b'#' && (bytes[idx - 1] == b' ' || bytes[idx - 1] == b'\t') {
            cut = idx;
            break;
        }
    }
    s[..cut].trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(input: &str) -> Vec<(String, String)> {
        parse(input).expect("parse")
    }
    fn val<'a>(pairs: &'a [(String, String)], key: &str) -> Option<&'a str> {
        pairs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn basic_assignments_and_comments() {
        let r = p("# comment\nFOO=bar\n\nBAZ=qux # inline\n");
        assert_eq!(val(&r, "FOO"), Some("bar"));
        assert_eq!(val(&r, "BAZ"), Some("qux"));
    }

    #[test]
    fn export_prefix_and_whitespace_around_equals() {
        let r = p("export FOO = bar \n");
        assert_eq!(val(&r, "FOO"), Some("bar"));
    }

    #[test]
    fn double_quotes_process_escapes() {
        let r = p(r#"FOO="a\nb\t\"c\"""#);
        assert_eq!(val(&r, "FOO"), Some("a\nb\t\"c\""));
    }

    #[test]
    fn single_quotes_are_literal() {
        let r = p(r#"FOO='a\nb $X #notcomment'"#);
        assert_eq!(val(&r, "FOO"), Some(r"a\nb $X #notcomment"));
    }

    #[test]
    fn hash_inside_value_is_kept_without_leading_space() {
        let r = p("FOO=ab#cd\n");
        assert_eq!(val(&r, "FOO"), Some("ab#cd"));
    }

    #[test]
    fn no_variable_expansion() {
        let r = p("A=1\nB=${A}-$A\n");
        assert_eq!(val(&r, "B"), Some("${A}-$A"));
    }

    #[test]
    fn multiline_double_quoted_value() {
        let r = p("KEY=\"line1\nline2\"\nNEXT=ok\n");
        assert_eq!(val(&r, "KEY"), Some("line1\nline2"));
        assert_eq!(val(&r, "NEXT"), Some("ok"));
    }

    #[test]
    fn crlf_and_bom_are_tolerated() {
        let r = p("\u{feff}FOO=bar\r\nBAZ=qux\r\n");
        assert_eq!(val(&r, "FOO"), Some("bar"));
        assert_eq!(val(&r, "BAZ"), Some("qux"));
    }

    #[test]
    fn empty_value() {
        let r = p("FOO=\nBAR=   # only a comment\n");
        assert_eq!(val(&r, "FOO"), Some(""));
        assert_eq!(val(&r, "BAR"), Some(""));
    }

    #[test]
    fn duplicate_keys_are_returned_in_order() {
        let r = p("K=first\nK=second\n");
        assert_eq!(r.len(), 2);
        assert_eq!(r[1], ("K".to_string(), "second".to_string()));
    }

    #[test]
    fn invalid_key_and_unterminated_quote_error_without_value() {
        let bad_key = parse("1ABC=x\n").unwrap_err();
        assert_eq!(bad_key.line, 1);
        assert!(bad_key.message.contains("invalid key"));

        let missing_eq = parse("JUSTSOMETEXT\n").unwrap_err();
        assert!(missing_eq.message.contains("KEY=value"));

        let unterminated = parse("K=\"oops\n").unwrap_err();
        assert!(unterminated.message.contains("unterminated"));
        // The error must not leak the value.
        assert!(!unterminated.message.contains("oops"));
    }
}
