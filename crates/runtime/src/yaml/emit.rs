//! The runtime's [`Value`] tree to YAML text.
//!
//! Block style throughout, two-space indentation, and sequences under a key
//! written at the key's own indentation — the layout `serde_yaml` produced,
//! kept so output does not change shape. A string is written plain when that
//! reads back as the same string, single-quoted when it would not, and
//! double-quoted when it holds something only an escape can carry. Multi-line
//! strings are literal block scalars when that round-trips exactly.

use super::compose::{parse_float, parse_int, resolve_plain};
use es_runtime_engine::Value;

/// Longest implicit key the specification allows; a longer one is written
/// with an explicit `? ` indicator.
const MAX_IMPLICIT_KEY: usize = 1024;

pub(crate) fn emit(value: &Value) -> String {
    let mut out = String::new();
    match value {
        Value::Array(items) if !items.is_empty() => seq(&mut out, items, 0, false),
        Value::Object(entries) if !entries.is_empty() => map(&mut out, entries, 0, false),
        Value::String(s) if literal_ok(s) => literal(&mut out, s, 2),
        other => {
            out.push_str(&inline(other));
            out.push('\n');
        }
    }
    out
}

fn indent(out: &mut String, n: usize) {
    out.extend(std::iter::repeat_n(' ', n));
}

fn is_block(v: &Value) -> bool {
    match v {
        Value::Array(items) => !items.is_empty(),
        Value::Object(entries) => !entries.is_empty(),
        Value::String(s) => literal_ok(s),
        _ => false,
    }
}

/// A sequence at `level`. `continued` means the first entry continues a line
/// already started (after `- `).
fn seq(out: &mut String, items: &[Value], level: usize, continued: bool) {
    for (i, item) in items.iter().enumerate() {
        if i > 0 || !continued {
            indent(out, level);
        }
        out.push('-');
        match item {
            Value::Array(inner) if !inner.is_empty() => {
                out.push(' ');
                seq(out, inner, level + 2, true);
            }
            Value::Object(inner) if !inner.is_empty() => {
                out.push(' ');
                map(out, inner, level + 2, true);
            }
            Value::String(s) if literal_ok(s) => {
                out.push(' ');
                literal(out, s, level + 2);
            }
            other => {
                out.push(' ');
                out.push_str(&inline(other));
                out.push('\n');
            }
        }
    }
}

fn map(out: &mut String, entries: &[(String, Value)], level: usize, continued: bool) {
    for (i, (key, value)) in entries.iter().enumerate() {
        if i > 0 || !continued {
            indent(out, level);
        }
        let key_text = string(key, true);
        if key_text.chars().count() > MAX_IMPLICIT_KEY {
            out.push_str("? ");
            out.push_str(&key_text);
            out.push('\n');
            indent(out, level);
        } else {
            out.push_str(&key_text);
        }
        out.push(':');
        match value {
            // A sequence under a key sits at the key's indentation.
            Value::Array(inner) if !inner.is_empty() => {
                out.push('\n');
                seq(out, inner, level, false);
            }
            Value::Object(inner) if !inner.is_empty() => {
                out.push('\n');
                map(out, inner, level + 2, false);
            }
            Value::String(s) if literal_ok(s) => {
                out.push(' ');
                literal(out, s, level + 2);
            }
            other => {
                debug_assert!(!is_block(other));
                out.push(' ');
                out.push_str(&inline(other));
                out.push('\n');
            }
        }
    }
}

/// A value that fits on the current line.
fn inline(v: &Value) -> String {
    match v {
        Value::Bool(true) => "true".into(),
        Value::Bool(false) => "false".into(),
        Value::Number(n) => number(*n),
        Value::String(s) => string(s, false),
        Value::Array(_) => "[]".into(),
        Value::Object(_) => "{}".into(),
        // A `BigInt` is a core-schema integer, exactly as written.
        Value::BigInt(i) => i.to_string(),
        // `undefined` is written as `null`, its nearest YAML value. Anything
        // else was refused by `check` before the walk began.
        _ => "null".into(),
    }
}

/// Refuses a value YAML has no form for — bytes, a function or symbol — instead
/// of writing it as `null` and losing it. Run before [`emit`], which then has
/// nothing left to refuse.
pub(crate) fn check(v: &Value) -> Result<(), String> {
    match v {
        Value::Null
        | Value::Undefined
        | Value::Bool(_)
        | Value::Number(_)
        | Value::BigInt(_)
        | Value::String(_) => Ok(()),
        Value::Array(items) => items.iter().try_for_each(check),
        Value::Object(entries) => entries.iter().try_for_each(|(_, v)| check(v)),
        Value::Bytes(_) => Err("YAML has no form for bytes (encode them as a string first)".into()),
        _ => Err("a function, symbol or similar value has no YAML form".into()),
    }
}

fn number(n: f64) -> String {
    if n.is_nan() {
        ".nan".into()
    } else if n.is_infinite() {
        if n > 0.0 {
            ".inf".into()
        } else {
            "-.inf".into()
        }
    } else {
        // `String(n)`: integral values without `.0` or `-0`, the shortest
        // round-trip digits, exponent form past 1e21 and below 1e-6 — all of
        // which the core schema reads back. An `i64` cast here used to saturate,
        // writing 2^63 as `9223372036854775807`.
        let s = crate::js_number::to_js_string(n);
        debug_assert!(parse_float(&s).is_some() || parse_int(&s).is_some(), "{s}");
        s
    }
}

/// Whether `s`, written plain, would read back as something other than this
/// string under the core schema — or under YAML 1.1, which many readers still
/// implement and which reads `yes`/`no`/`on`/`off` as booleans.
fn plain_resolves_elsewhere(s: &str) -> bool {
    !matches!(resolve_plain(s), Value::String(_))
        || matches!(
            s,
            "y" | "Y"
                | "yes"
                | "Yes"
                | "YES"
                | "n"
                | "N"
                | "no"
                | "No"
                | "NO"
                | "on"
                | "On"
                | "ON"
                | "off"
                | "Off"
                | "OFF"
        )
}

/// Characters that only a double-quoted escape can carry. Tab and line feed
/// are not among them: a literal block scalar holds both.
fn needs_escape(c: char) -> bool {
    matches!(c, '\0'..='\u{8}' | '\u{b}'..='\u{1f}' | '\u{7f}'..='\u{9f}' | '\u{feff}' | '\u{2028}' | '\u{2029}' | '\u{fffe}' | '\u{ffff}')
}

fn plain_ok(s: &str) -> bool {
    let Some(first) = s.chars().next() else {
        return false;
    };
    let last = s.chars().next_back().unwrap();
    if plain_resolves_elsewhere(s)
        || matches!(
            first,
            '-' | '?'
                | ':'
                | ','
                | '['
                | ']'
                | '{'
                | '}'
                | '#'
                | '&'
                | '*'
                | '!'
                | '|'
                | '>'
                | '\''
                | '"'
                | '%'
                | '@'
                | '`'
        )
        || first == ' '
        || last == ' '
        || last == ':'
        || s.starts_with("...")
        || s.contains(": ")
        || s.contains(" #")
        || s.chars().any(|c| c == '\t' || c == '\n' || needs_escape(c))
    {
        return false;
    }
    true
}

fn string(s: &str, key: bool) -> String {
    // A plain `<<` key would read back as a merge.
    if plain_ok(s) && !(key && s == "<<") {
        return s.to_string();
    }
    if s.chars()
        .any(|c| needs_escape(c) || c == '\t' || (key && c == '\n'))
        || s.contains('\n')
    {
        return double_quoted(s);
    }
    format!("'{}'", s.replace('\'', "''"))
}

fn double_quoted(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\0' => out.push_str("\\0"),
            '\u{7}' => out.push_str("\\a"),
            '\u{8}' => out.push_str("\\b"),
            '\t' => out.push_str("\\t"),
            '\n' => out.push_str("\\n"),
            '\u{b}' => out.push_str("\\v"),
            '\u{c}' => out.push_str("\\f"),
            '\r' => out.push_str("\\r"),
            '\u{1b}' => out.push_str("\\e"),
            '\u{85}' => out.push_str("\\N"),
            '\u{2028}' => out.push_str("\\L"),
            '\u{2029}' => out.push_str("\\P"),
            c if needs_escape(c) => {
                let n = c as u32;
                if n <= 0xff {
                    out.push_str(&format!("\\x{n:02X}"));
                } else {
                    out.push_str(&format!("\\u{n:04X}"));
                }
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Whether a literal block scalar (`|`) reproduces `s` exactly: it must span
/// lines, have content, need no escapes, and keep no trailing whitespace
/// (which editors strip). Its first content line must not start with a space,
/// or the reader would take that space as indentation.
fn literal_ok(s: &str) -> bool {
    if !s.contains('\n') || s.chars().any(needs_escape) {
        return false;
    }
    let body = s.trim_end_matches('\n');
    if body.is_empty() {
        return false;
    }
    let first_content = body.lines().find(|l| !l.is_empty());
    if first_content.is_some_and(|l| l.starts_with(' ') || l.starts_with('\t')) {
        return false;
    }
    !body.lines().any(|l| l.ends_with(' ') || l.ends_with('\t'))
}

fn literal(out: &mut String, s: &str, level: usize) {
    let body = s.trim_end_matches('\n');
    let trailing = s.len() - body.len();
    out.push_str(match trailing {
        0 => "|-",
        1 => "|",
        _ => "|+",
    });
    out.push('\n');
    for line in body.split('\n') {
        if !line.is_empty() {
            indent(out, level);
            out.push_str(line);
        }
        out.push('\n');
    }
    for _ in 1..trailing {
        out.push('\n');
    }
}
