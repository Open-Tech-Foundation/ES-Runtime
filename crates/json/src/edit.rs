//! Changing one value in a JSONC or JSON5 text while keeping every other byte
//! (DECISIONS D191).
//!
//! An edit works on the text, not on a value: it finds the member the path
//! names, by the same scanners the parser uses, and splices. Comments, blank
//! lines, key order, quotes and indentation outside the change stay as they
//! were. The new value arrives already written in the dialect (the caller's
//! `stringify`, in the style [`plan`] reports), and is re-indented to its
//! place here.
//!
//! - An existing member's value is replaced where it stands. Of a repeated
//!   key, the last is the one that counts, so it is the one replaced.
//! - A missing key is appended to its object in the style of its siblings:
//!   their indent, the space after their colon, their trailing comma, and, in
//!   JSON5, their quoting. Missing objects on the path are created.
//! - An index equal to an array's length appends to it.
//! - A removal takes the member, its comma, and a comment on the rest of its
//!   line; comments on the lines above stay. Every occurrence of a repeated
//!   key is removed, so the key is gone.

use std::borrow::Cow;

use crate::lexer::{Lexed, Lexer};
use crate::{Dialect, Error, Text, validate};

/// One step of a path: a property name, or an array index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment<'a> {
    /// A property of an object.
    Key(Cow<'a, str>),
    /// An element of an array; in an object, the key of that number.
    Index(usize),
}

/// How the caller should write the new value so it fits where it goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The indent to `stringify` with: the file's indent unit where the value
    /// goes on its own lines, and empty where it goes inline.
    pub indent: String,
    /// JSON5's quote for strings, from the file; `None` when it has none, or
    /// in JSONC, whose strings are always double-quoted.
    pub quote: Option<char>,
    /// Whether the file ends a multi-line object's or array's last member with
    /// a comma, so the new value's own containers should too.
    pub trailing_commas: bool,
}

/// Why an edit was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditError {
    /// The text is not in the dialect, so nothing is changed: a `SyntaxError`.
    Syntax(Box<Error>),
    /// The path goes through something that cannot hold it: a `TypeError`.
    Path(String),
    /// An index past the end of an array: a `RangeError`.
    Range(String),
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EditError::Syntax(e) => e.fmt(f),
            EditError::Path(m) | EditError::Range(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for EditError {}

impl From<Box<Error>> for EditError {
    fn from(e: Box<Error>) -> Self {
        EditError::Syntax(e)
    }
}

/// How [`edit`]'s caller should write the value for `path`.
pub fn plan(text: &str, dialect: Dialect, path: &[Segment<'_>]) -> Result<Plan, EditError> {
    validate(text, dialect).map_err(|e| EditError::Syntax(Box::new(e)))?;
    let doc = Doc::new(text, dialect)?;
    let target = doc.locate(path)?;
    let inline = match &target {
        Target::Root { .. } => !text.contains('\n'),
        Target::Member { container, .. } | Target::Missing { container, .. } => {
            !doc.multi_line(container)
        }
    };
    Ok(Plan {
        indent: if inline {
            String::new()
        } else {
            doc.style.unit.clone()
        },
        quote: doc.style.quote.filter(|_| dialect == Dialect::Json5),
        trailing_commas: doc.style.trailing_commas,
    })
}

/// `text` with the value at `path` set to `value`, written in the dialect as
/// [`plan`] asked, or removed when `value` is `None`.
pub fn edit(
    text: &str,
    dialect: Dialect,
    path: &[Segment<'_>],
    value: Option<&str>,
) -> Result<String, EditError> {
    validate(text, dialect).map_err(|e| EditError::Syntax(Box::new(e)))?;
    if let Some(value) = value {
        // The value must be one value in the dialect, or the result would not be.
        validate(value, dialect).map_err(|e| {
            EditError::Path(format!(
                "the new value is not {}: {e}",
                dialect_name(dialect)
            ))
        })?;
    }
    let doc = Doc::new(text, dialect)?;
    match value {
        Some(value) => doc.set(path, value),
        None => {
            if path.is_empty() {
                return Err(EditError::Path(
                    "the whole document cannot be removed".into(),
                ));
            }
            // Each occurrence of a repeated key, last first, until none is
            // left. An array element is removed once: the next one moves into
            // its index.
            let mut text = Cow::Borrowed(text);
            loop {
                let doc = Doc::new(&text, dialect)?;
                match doc.remove(path)? {
                    Some((next, true)) => text = Cow::Owned(next),
                    Some((next, false)) => return Ok(next),
                    None => return Ok(text.into_owned()),
                }
            }
        }
    }
}

fn dialect_name(dialect: Dialect) -> &'static str {
    match dialect {
        Dialect::Jsonc => "JSONC",
        Dialect::Json5 => "JSON5",
    }
}

// ---- the document's shape ------------------------------------------------------

/// A member of an object or an element of an array, by byte offsets.
#[derive(Debug, Clone)]
struct Member<'a> {
    /// Where the member starts: its key, or for an element its value.
    start: usize,
    key: Option<(Text<'a>, usize)>,
    value_start: usize,
    value_end: usize,
    /// The comma after the value, if any.
    comma: Option<usize>,
}

#[derive(Debug, Clone)]
struct Container<'a> {
    object: bool,
    open: usize,
    close: usize,
    members: Vec<Member<'a>>,
}

enum Target<'a> {
    /// The path is empty: the whole value.
    Root { start: usize, end: usize },
    /// The path names this member of this container.
    Member {
        container: Container<'a>,
        index: usize,
    },
    /// The path stops existing at `depth`: `container` holds the path's first
    /// `depth` steps and has no `path[depth]`.
    Missing {
        container: Container<'a>,
        depth: usize,
    },
}

/// The file's habits, which a new member copies.
#[derive(Debug)]
struct Style {
    unit: String,
    newline: &'static str,
    colon: String,
    separator: String,
    trailing_commas: bool,
    quote: Option<char>,
    bare_keys: bool,
}

struct Doc<'a> {
    text: &'a str,
    dialect: Dialect,
    style: Style,
}

impl<'a> Doc<'a> {
    fn new(text: &'a str, dialect: Dialect) -> Lexed<Doc<'a>> {
        let mut doc = Doc {
            text,
            dialect,
            style: Style {
                unit: String::new(),
                newline: if text.contains("\r\n") { "\r\n" } else { "\n" },
                colon: String::new(),
                separator: String::new(),
                trailing_commas: false,
                quote: None,
                bare_keys: dialect == Dialect::Json5,
            },
        };
        let mut seen = Seen::default();
        let mut lexer = doc.lexer();
        lexer.peek()?;
        let root = lexer.pos();
        doc.survey(root, &mut seen)?;
        let style = &mut doc.style;
        style.unit = seen.unit.unwrap_or_else(|| "  ".into());
        style.colon = seen.colon.unwrap_or_else(|| ": ".into());
        style.separator = seen.separator.unwrap_or_else(|| " ".into());
        style.trailing_commas = seen.trailing_comma;
        style.quote = seen.quote;
        if let Some(bare) = seen.bare_keys {
            style.bare_keys = bare;
        }
        Ok(doc)
    }

    fn lexer(&self) -> Lexer<'a> {
        Lexer::new(self.text, self.dialect)
    }

    /// The container whose opening bracket is at `open`.
    fn container(&self, open: usize) -> Lexed<Container<'a>> {
        let mut lexer = self.lexer();
        lexer.seek(open);
        let object = lexer.peek()? == Some(b'{');
        lexer.bump();
        let mut members = Vec::new();
        loop {
            let b = lexer.peek()?.expect("the text is valid");
            if b == b'}' || b == b']' {
                let close = lexer.pos();
                return Ok(Container {
                    object,
                    open,
                    close,
                    members,
                });
            }
            let start = lexer.pos();
            let key = if object {
                let key = match b {
                    b'"' | b'\'' => lexer.string(b)?,
                    _ => lexer.word()?.0,
                };
                let key_end = lexer.pos();
                lexer.peek()?;
                lexer.bump();
                Some((key, key_end))
            } else {
                None
            };
            lexer.peek()?;
            let value_start = lexer.pos();
            skip_value(&mut lexer)?;
            let value_end = lexer.pos();
            let comma = if lexer.peek()? == Some(b',') {
                let at = lexer.pos();
                lexer.bump();
                Some(at)
            } else {
                None
            };
            members.push(Member {
                start,
                key,
                value_start,
                value_end,
                comma,
            });
        }
    }

    /// Records the file's habits from every container in the value at `at`.
    fn survey(&self, at: usize, seen: &mut Seen) -> Lexed<()> {
        let first = self.text.as_bytes()[at];
        if first == b'"' || first == b'\'' {
            seen.quote.get_or_insert(char::from(first));
        }
        if first != b'{' && first != b'[' {
            return Ok(());
        }
        let container = self.container(at)?;
        let multi = self.multi_line(&container);
        if multi && seen.unit.is_none() {
            let outer = self.line_indent(container.open);
            seen.unit = container
                .members
                .iter()
                .filter(|m| self.starts_line(m.start))
                .find_map(|m| {
                    let inner = self.line_indent(m.start);
                    inner
                        .strip_prefix(outer)
                        .filter(|unit| !unit.is_empty())
                        .map(str::to_owned)
                });
        }
        if multi && container.members.last().is_some_and(|m| m.comma.is_some()) {
            seen.trailing_comma = true;
        }
        if !multi && seen.separator.is_none() && container.members.len() >= 2 {
            let comma = container.members[0]
                .comma
                .expect("a member before another has a comma");
            let gap = &self.text[comma + 1..container.members[1].start];
            if gap.chars().all(|c| c == ' ' || c == '\t') {
                seen.separator = Some(gap.to_owned());
            }
        }
        for member in &container.members {
            if let Some((_, key_end)) = &member.key {
                let quote = self.text.as_bytes()[member.start];
                if quote == b'"' || quote == b'\'' {
                    seen.quote.get_or_insert(char::from(quote));
                    seen.bare_keys.get_or_insert(false);
                } else {
                    seen.bare_keys.get_or_insert(true);
                }
                let gap = &self.text[*key_end..member.value_start];
                if seen.colon.is_none() && !gap.contains('/') && !gap.contains('\n') {
                    seen.colon = Some(gap.to_owned());
                }
            }
            self.survey(member.value_start, seen)?;
        }
        Ok(())
    }

    fn multi_line(&self, container: &Container<'_>) -> bool {
        if container.members.is_empty() {
            // An empty container takes the layout of the file around it.
            return self.text.contains('\n');
        }
        self.text[container.open..container.close].contains('\n')
    }

    /// The whitespace that starts the line holding `at`.
    fn line_indent(&self, at: usize) -> &'a str {
        let start = self.line_start(at);
        let rest = &self.text[start..];
        let len = rest.len() - rest.trim_start_matches([' ', '\t']).len();
        &rest[..len]
    }

    fn line_start(&self, at: usize) -> usize {
        self.text[..at].rfind(['\n', '\r']).map_or(0, |i| i + 1)
    }

    /// Whether only whitespace precedes `at` on its line.
    fn starts_line(&self, at: usize) -> bool {
        self.text[self.line_start(at)..at]
            .chars()
            .all(|c| c == ' ' || c == '\t')
    }

    /// The end of `at`'s line when nothing but spaces and a comment follow
    /// `at` on it: the offset of the line break. `None` when something else
    /// follows on the same line.
    fn rest_of_line(&self, at: usize) -> Option<usize> {
        let bytes = self.text.as_bytes();
        let mut i = at;
        loop {
            match bytes.get(i) {
                None => return Some(i),
                Some(b' ' | b'\t') => i += 1,
                Some(b'\n' | b'\r') => return Some(i),
                Some(b'/') if bytes.get(i + 1) == Some(&b'/') => {
                    return Some(
                        self.text[i..]
                            .find(['\n', '\r'])
                            .map_or(self.text.len(), |n| i + n),
                    );
                }
                Some(b'/') if bytes.get(i + 1) == Some(&b'*') => {
                    let end = i + 2 + self.text[i + 2..].find("*/")? + 2;
                    if self.text[i..end].contains(['\n', '\r']) {
                        return None;
                    }
                    i = end;
                }
                Some(_) => return None,
            }
        }
    }

    /// The offset just past the line break at `at`, if there is one.
    fn past_line_break(&self, at: usize) -> usize {
        let rest = &self.text[at..];
        if rest.starts_with("\r\n") {
            at + 2
        } else if rest.starts_with(['\n', '\r']) {
            at + 1
        } else {
            at
        }
    }

    // ---- finding the path --------------------------------------------------------

    fn locate(&self, path: &[Segment<'_>]) -> Result<Target<'a>, EditError> {
        let mut lexer = self.lexer();
        lexer.peek()?;
        let mut at = lexer.pos();
        if path.is_empty() {
            skip_value(&mut lexer)?;
            return Ok(Target::Root {
                start: at,
                end: lexer.pos(),
            });
        }
        for (depth, segment) in path.iter().enumerate() {
            let container = match self.text.as_bytes()[at] {
                b'{' | b'[' => self.container(at)?,
                _ => {
                    return Err(EditError::Path(format!(
                        "{} is {}, which has no {}",
                        describe_path(&path[..depth]),
                        self.describe_value(at),
                        describe_segment(segment),
                    )));
                }
            };
            let found = match (segment, container.object) {
                (Segment::Key(key), true) => self.find_key(&container, key),
                (Segment::Index(i), true) => self.find_key(&container, &i.to_string()),
                (Segment::Index(i), false) => {
                    let len = container.members.len();
                    if *i > len {
                        let elements = if len == 1 { "element" } else { "elements" };
                        return Err(EditError::Range(format!(
                            "{} has {len} {elements}, so [{i}] is past its end",
                            describe_path(&path[..depth]),
                        )));
                    }
                    (*i < container.members.len()).then_some(*i)
                }
                (Segment::Key(key), false) => {
                    return Err(EditError::Path(format!(
                        "{} is an array, which has no property \"{key}\"",
                        describe_path(&path[..depth])
                    )));
                }
            };
            let Some(index) = found else {
                return Ok(Target::Missing { container, depth });
            };
            if depth + 1 == path.len() {
                return Ok(Target::Member { container, index });
            }
            at = container.members[index].value_start;
        }
        unreachable!("a non-empty path returns from its last step")
    }

    /// The last member with `key`, since the last one is the one that counts.
    fn find_key(&self, container: &Container<'_>, key: &str) -> Option<usize> {
        container.members.iter().rposition(|m| match &m.key {
            Some((Text::Str(name), _)) => name == key,
            Some((Text::Utf16(units), _)) => units.iter().copied().eq(key.encode_utf16()),
            None => false,
        })
    }

    fn describe_value(&self, at: usize) -> &'static str {
        match self.text.as_bytes()[at] {
            b'"' | b'\'' => "a string",
            b't' | b'f' => "a boolean",
            b'n' => "null",
            _ => "a number",
        }
    }

    // ---- writing -------------------------------------------------------------------

    /// `value`, written for column 0 with `\n` line breaks, re-indented to sit
    /// at a line indented by `base`, with the file's line breaks.
    fn place(&self, value: &str, base: &str) -> String {
        let mut out = String::with_capacity(value.len());
        for (i, line) in value.split('\n').enumerate() {
            if i > 0 {
                out.push_str(self.style.newline);
                out.push_str(base);
            }
            out.push_str(line);
        }
        out
    }

    fn key(&self, key: &str) -> String {
        if self.dialect == Dialect::Json5 && self.style.bare_keys && is_identifier(key) {
            return key.to_owned();
        }
        let quote = match self.dialect {
            Dialect::Json5 => self.style.quote.unwrap_or('"'),
            Dialect::Jsonc => '"',
        };
        quote_string(key, quote)
    }

    /// The value for the path's steps after `depth`: `value` inside the
    /// objects that do not exist yet, outermost first.
    fn wrap(
        &self,
        path: &[Segment<'_>],
        depth: usize,
        value: &str,
        inline: bool,
    ) -> Result<String, EditError> {
        let mut out = value.to_owned();
        for segment in path[depth + 1..].iter().rev() {
            let key = match segment {
                Segment::Key(key) => key.to_string(),
                Segment::Index(i) => {
                    return Err(EditError::Path(format!(
                        "{} does not exist, and an array is not created for [{i}]",
                        describe_path(&path[..=depth])
                    )));
                }
            };
            let member = format!("{}{}{}", self.key(&key), self.style.colon, out);
            out = if inline {
                format!("{{{member}}}")
            } else {
                let comma = if self.style.trailing_commas { "," } else { "" };
                let unit = &self.style.unit;
                let member = member.replace('\n', &format!("\n{unit}"));
                format!("{{\n{unit}{member}{comma}\n}}")
            };
        }
        Ok(out)
    }

    fn set(&self, path: &[Segment<'_>], value: &str) -> Result<String, EditError> {
        let text = self.text;
        match self.locate(path)? {
            Target::Root { start, end } => {
                let placed = self.place(value, self.line_indent(start));
                Ok(format!("{}{placed}{}", &text[..start], &text[end..]))
            }
            Target::Member { container, index } => {
                let member = &container.members[index];
                let placed = self.place(value, self.line_indent(member.start));
                Ok(format!(
                    "{}{placed}{}",
                    &text[..member.value_start],
                    &text[member.value_end..]
                ))
            }
            Target::Missing { container, depth } => {
                let inline = !self.multi_line(&container);
                let value = self.wrap(path, depth, value, inline)?;
                let entry = |indent: &str| {
                    let value = self.place(&value, indent);
                    match &path[depth] {
                        _ if !container.object => value,
                        Segment::Key(key) => {
                            format!("{}{}{value}", self.key(key), self.style.colon)
                        }
                        Segment::Index(i) => {
                            format!("{}{}{value}", self.key(&i.to_string()), self.style.colon)
                        }
                    }
                };
                Ok(self.insert(&container, inline, entry))
            }
        }
    }

    /// `container` with a new last member, written by `entry` for the indent
    /// of the line it starts.
    fn insert(
        &self,
        container: &Container<'_>,
        inline: bool,
        entry: impl Fn(&str) -> String,
    ) -> String {
        let text = self.text;
        let nl = self.style.newline;
        let Some(last) = container.members.last() else {
            let interior = &text[container.open + 1..container.close];
            let blank = interior.trim_matches([' ', '\t', '\n', '\r']).is_empty();
            if inline {
                let (from, to) = if blank {
                    (container.open + 1, container.close)
                } else {
                    (container.close, container.close)
                };
                return format!("{}{}{}", &text[..from], entry(""), &text[to..]);
            }
            let outer = self.line_indent(container.open);
            let inner = format!("{outer}{}", self.style.unit);
            let comma = if self.style.trailing_commas { "," } else { "" };
            if blank {
                return format!(
                    "{}{nl}{inner}{}{comma}{nl}{outer}{}",
                    &text[..=container.open],
                    entry(&inner),
                    &text[container.close..]
                );
            }
            // Comments inside: they stay, and the member follows them.
            let at = text[..container.close]
                .trim_end_matches([' ', '\t', '\n', '\r'])
                .len();
            return format!(
                "{}{nl}{inner}{}{comma}{nl}{outer}{}",
                &text[..at],
                entry(&inner),
                &text[container.close..]
            );
        };
        if inline {
            let separator = &self.style.separator;
            return match last.comma {
                Some(comma) => format!(
                    "{}{separator}{},{}",
                    &text[..=comma],
                    entry(""),
                    &text[comma + 1..]
                ),
                None => format!(
                    "{},{separator}{}{}",
                    &text[..last.value_end],
                    entry(""),
                    &text[last.value_end..]
                ),
            };
        }
        let indent = if self.starts_line(last.start) {
            self.line_indent(last.start).to_owned()
        } else {
            format!("{}{}", self.line_indent(container.open), self.style.unit)
        };
        let after = last.comma.map_or(last.value_end, |c| c + 1);
        // After the rest of the last member's line, so its comment stays on it.
        let at = self.rest_of_line(after).unwrap_or(after);
        let trailing = if last.comma.is_some() { "," } else { "" };
        let entry = format!("{nl}{indent}{}{trailing}", entry(&indent));
        match last.comma {
            Some(_) => format!("{}{entry}{}", &text[..at], &text[at..]),
            None => format!(
                "{},{}{entry}{}",
                &text[..last.value_end],
                &text[last.value_end..at],
                &text[at..]
            ),
        }
    }

    /// The text with the path's member removed, and whether it was an
    /// object's (whose key may repeat); `None` when it is absent.
    fn remove(&self, path: &[Segment<'_>]) -> Result<Option<(String, bool)>, EditError> {
        let (container, index) = match self.locate(path) {
            Ok(Target::Member { container, index }) => (container, index),
            Ok(_) | Err(EditError::Range(_)) => return Ok(None),
            Err(e) => return Err(e),
        };
        let text = self.text;
        let members = &container.members;
        let member = &members[index];
        let last = index + 1 == members.len();
        // A last member without a comma takes the comma before it, so the one
        // before does not become a trailing comma the file did not have.
        let previous_comma = (last && member.comma.is_none() && index > 0)
            .then(|| members[index - 1].comma)
            .flatten();
        let end = member.comma.map_or(member.value_end, |c| c + 1);
        if self.starts_line(member.start)
            && let Some(line_end) = self.rest_of_line(end)
        {
            // The member has its lines to itself: they go, line break included.
            let from = self.line_start(member.start);
            let to = self.past_line_break(line_end);
            let text = match previous_comma {
                Some(comma) => format!(
                    "{}{}{}",
                    &text[..comma],
                    &text[comma + 1..from],
                    &text[to..]
                ),
                None => format!("{}{}", &text[..from], &text[to..]),
            };
            return Ok(Some((text, container.object)));
        }
        let text = match (member.comma, previous_comma) {
            (Some(comma), _) => {
                let rest = &text[comma + 1..];
                let spaces = rest.len() - rest.trim_start_matches([' ', '\t']).len();
                format!("{}{}", &text[..member.start], &text[comma + 1 + spaces..])
            }
            (None, Some(comma)) => format!("{}{}", &text[..comma], &text[member.value_end..]),
            (None, None) => format!("{}{}", &text[..member.start], &text[member.value_end..]),
        };
        Ok(Some((text, container.object)))
    }
}

#[derive(Default)]
struct Seen {
    unit: Option<String>,
    colon: Option<String>,
    separator: Option<String>,
    trailing_comma: bool,
    quote: Option<char>,
    bare_keys: Option<bool>,
}

/// Steps over one value, whatever it is.
fn skip_value(lexer: &mut Lexer<'_>) -> Lexed<()> {
    let mut depth = 0usize;
    loop {
        let b = lexer.peek()?.expect("the text is valid");
        match b {
            b'{' | b'[' => {
                depth += 1;
                lexer.bump();
            }
            b'}' | b']' => {
                depth -= 1;
                lexer.bump();
            }
            b',' | b':' => lexer.bump(),
            b'"' | b'\'' => {
                lexer.string(b)?;
            }
            b'-' | b'+' | b'.' | b'0'..=b'9' => {
                lexer.number()?;
            }
            _ => {
                lexer.word()?;
            }
        }
        if depth == 0 {
            return Ok(());
        }
    }
}

fn describe_segment(segment: &Segment<'_>) -> String {
    match segment {
        Segment::Key(key) => format!("property \"{key}\""),
        Segment::Index(i) => format!("element [{i}]"),
    }
}

fn describe_path(path: &[Segment<'_>]) -> String {
    let mut out = String::from("the value at ");
    if path.is_empty() {
        out.push_str("the top");
        return out;
    }
    for segment in path {
        match segment {
            Segment::Key(key) => {
                out.push('.');
                out.push_str(key);
            }
            Segment::Index(i) => out.push_str(&format!("[{i}]")),
        }
    }
    out
}

/// An identifier every JSON5 reader leaves unquoted: ASCII only (D191).
fn is_identifier(key: &str) -> bool {
    let mut bytes = key.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'$' || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'$' || b == b'_')
}

/// `text` as a string literal in `quote`.
fn quote_string(text: &str, quote: char) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push(quote);
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{8}' => out.push_str("\\b"),
            '\u{c}' => out.push_str("\\f"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if u32::from(c) < 0x20 => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}
