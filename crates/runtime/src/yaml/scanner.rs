//! Characters to tokens.
//!
//! The token set and the simple-key machinery are libyaml's, which is the
//! shape every conforming YAML implementation has converged on: indentation is
//! turned into explicit `BlockMappingStart`/`BlockSequenceStart`/`BlockEnd`
//! tokens, and a `Key` token is inserted *retroactively* when a `:` turns out
//! to follow something that could have been an implicit key. Where libyaml
//! follows YAML 1.1 and 1.2 differs — what may start a plain scalar, when a
//! `:` is a value indicator inside flow collections, where tabs and comments
//! may appear — this follows 1.2.

use super::{Error, Mark};
use std::collections::VecDeque;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScalarStyle {
    Plain,
    SingleQuoted,
    DoubleQuoted,
    Literal,
    Folded,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum TokenKind {
    StreamStart,
    StreamEnd,
    VersionDirective(u32, u32),
    /// `%TAG handle prefix`.
    TagDirective(String, String),
    /// Any other directive, which the specification says to ignore.
    ReservedDirective,
    DocumentStart,
    DocumentEnd,
    BlockSequenceStart,
    BlockMappingStart,
    BlockEnd,
    FlowSequenceStart,
    FlowSequenceEnd,
    FlowMappingStart,
    FlowMappingEnd,
    BlockEntry,
    FlowEntry,
    Key,
    Value,
    Alias(String),
    Anchor(String),
    /// A tag as written: the handle (`!`, `!!`, `!name!`, or empty for a
    /// verbatim `!<…>`) and the suffix, already percent-decoded.
    Tag(String, String),
    Scalar(String, ScalarStyle),
}

#[derive(Debug, Clone)]
pub(crate) struct Token {
    pub kind: TokenKind,
    pub start: Mark,
}

#[derive(Debug, Clone, Copy, Default)]
struct SimpleKey {
    possible: bool,
    required: bool,
    token_number: usize,
    mark: Mark,
}

/// Longest implicit key the specification allows, in characters.
const MAX_SIMPLE_KEY_LENGTH: usize = 1024;

pub(crate) struct Scanner {
    buf: Vec<char>,
    mark: Mark,
    tokens: VecDeque<Token>,
    tokens_taken: usize,
    started: bool,
    done: bool,
    indent: isize,
    indents: Vec<isize>,
    flow_level: usize,
    /// For each open flow collection, whether it is a mapping. An implicit key
    /// in a flow mapping may span lines; anywhere else it may not.
    flow_mappings: Vec<bool>,
    simple_key_allowed: bool,
    simple_keys: Vec<SimpleKey>,
    /// Set by a token after which a flow `:` needs no following space: a
    /// quoted scalar or the end of a flow collection (YAML 1.2's JSON-like
    /// keys, as in `{"a":1}`).
    json_like: bool,
    /// Whether the last token pushed ends a node (scalar, alias, flow
    /// collection end), and the line it ended on.
    last_node_end_line: Option<usize>,
    /// The line of the most recent `---`, on which no block collection may start.
    document_start_line: Option<usize>,
}

fn is_blank(c: char) -> bool {
    c == ' ' || c == '\t'
}

fn is_break(c: char) -> bool {
    c == '\n'
}

fn is_blankz(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\0')
}

fn is_breakz(c: char) -> bool {
    c == '\n' || c == '\0'
}

fn is_flow_indicator(c: char) -> bool {
    matches!(c, ',' | '[' | ']' | '{' | '}')
}

fn is_indicator(c: char) -> bool {
    matches!(
        c,
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
}

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-'
}

fn is_uri_char(c: char) -> bool {
    is_word_char(c) || "#;/?:@&=+$,_.!~*'()[]%".contains(c)
}

/// Whether a character may appear in a YAML stream at all (`c-printable`).
fn is_printable(c: char) -> bool {
    matches!(c,
        '\t' | '\n' | '\r'
        | '\u{20}'..='\u{7e}'
        | '\u{85}'
        | '\u{a0}'..='\u{d7ff}'
        | '\u{e000}'..='\u{fffd}'
        | '\u{10000}'..)
}

impl Scanner {
    pub fn new(input: &str) -> Result<Scanner, Error> {
        // Line breaks are normalized once, here: YAML 1.2 knows only LF, CRLF
        // and CR, and every scalar style folds them to LF anyway.
        let mut buf = Vec::with_capacity(input.len());
        let mut mark = Mark::default();
        let mut chars = input.chars().peekable();
        while let Some(c) = chars.next() {
            if !is_printable(c) {
                return Err(Error::syntax(
                    format!("character U+{:04X} is not allowed in YAML", c as u32),
                    mark,
                ));
            }
            if c == '\r' {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                buf.push('\n');
            } else {
                buf.push(c);
            }
            if c == '\n' || c == '\r' {
                mark.line += 1;
                mark.column = 0;
            } else {
                mark.column += 1;
            }
            mark.index += 1;
        }
        Ok(Scanner {
            buf,
            mark: Mark::default(),
            tokens: VecDeque::new(),
            tokens_taken: 0,
            started: false,
            done: false,
            indent: -1,
            indents: Vec::new(),
            flow_level: 0,
            flow_mappings: Vec::new(),
            simple_key_allowed: false,
            simple_keys: vec![SimpleKey::default()],
            json_like: false,
            last_node_end_line: None,
            document_start_line: None,
        })
    }

    fn ch(&self, k: usize) -> char {
        self.buf.get(self.mark.index + k).copied().unwrap_or('\0')
    }

    fn forward(&mut self) {
        if let Some(&c) = self.buf.get(self.mark.index) {
            self.mark.index += 1;
            if c == '\n' {
                self.mark.line += 1;
                self.mark.column = 0;
            } else {
                self.mark.column += 1;
            }
        }
    }

    fn forward_n(&mut self, n: usize) {
        for _ in 0..n {
            self.forward();
        }
    }

    fn col(&self) -> isize {
        self.mark.column as isize
    }

    fn error<T>(&self, message: impl Into<String>) -> Result<T, Error> {
        Err(Error::syntax(message, self.mark))
    }

    /// Whether `---` or `...` starts at the current position and is a marker.
    fn at_document_marker(&self) -> bool {
        self.mark.column == 0
            && ((self.ch(0) == '-' && self.ch(1) == '-' && self.ch(2) == '-')
                || (self.ch(0) == '.' && self.ch(1) == '.' && self.ch(2) == '.'))
            && is_blankz(self.ch(3))
    }

    /// Returns the next token without consuming it.
    pub fn peek(&mut self) -> Result<&Token, Error> {
        self.fetch_more()?;
        Ok(self
            .tokens
            .front()
            .expect("fetch_more leaves a token queued"))
    }

    pub fn next_token(&mut self) -> Result<Token, Error> {
        self.fetch_more()?;
        self.tokens_taken += 1;
        Ok(self
            .tokens
            .pop_front()
            .expect("fetch_more leaves a token queued"))
    }

    fn fetch_more(&mut self) -> Result<(), Error> {
        loop {
            let need = if self.tokens.is_empty() {
                !self.done
            } else {
                self.stale_simple_keys()?;
                let taken = self.tokens_taken;
                self.simple_keys
                    .iter()
                    .any(|k| k.possible && k.token_number == taken)
            };
            if !need {
                if self.tokens.is_empty() {
                    // Past the end: keep answering StreamEnd.
                    self.tokens.push_back(Token {
                        kind: TokenKind::StreamEnd,
                        start: self.mark,
                    });
                }
                return Ok(());
            }
            self.fetch_next()?;
        }
    }

    fn push(&mut self, kind: TokenKind, start: Mark) {
        self.last_node_end_line = match kind {
            TokenKind::Scalar(..)
            | TokenKind::Alias(_)
            | TokenKind::FlowSequenceEnd
            | TokenKind::FlowMappingEnd => Some(self.mark.line),
            _ => None,
        };
        self.tokens.push_back(Token { kind, start });
    }

    fn fetch_next(&mut self) -> Result<(), Error> {
        if !self.started {
            self.started = true;
            self.simple_key_allowed = true;
            if self.ch(0) == '\u{feff}' {
                self.forward();
            }
            self.push(TokenKind::StreamStart, self.mark);
            return Ok(());
        }
        let line_before = self.mark.line;
        self.scan_to_next_token()?;
        self.stale_simple_keys()?;
        self.unwind_indent(self.col());

        // A token that starts a line must be indented past the enclosing block
        // collection, and only spaces count as indentation: a tab may follow
        // them but never replaces them (YAML 1.2 `s-flow-line-prefix`).
        if self.mark.line != line_before && self.ch(0) != '\0' {
            let (spaces, tabs) = self.line_indentation();
            if spaces as isize <= self.indent && (tabs || self.flow_level > 0) {
                return self.error(if tabs {
                    "tabs are not allowed as indentation"
                } else {
                    "flow collection content is not indented enough"
                });
            }
        }

        let adjacent_value = std::mem::take(&mut self.json_like);
        let c = self.ch(0);
        let next = self.ch(1);
        match c {
            '\0' => self.fetch_stream_end(),
            '%' if self.mark.column == 0 => self.fetch_directive(),
            '-' | '.' if self.at_document_marker() => self.fetch_document_indicator(c == '-'),
            '[' => self.fetch_flow_collection_start(TokenKind::FlowSequenceStart),
            '{' => self.fetch_flow_collection_start(TokenKind::FlowMappingStart),
            ']' => self.fetch_flow_collection_end(TokenKind::FlowSequenceEnd),
            '}' => self.fetch_flow_collection_end(TokenKind::FlowMappingEnd),
            ',' => self.fetch_flow_entry(),
            '-' if is_blankz(next) => self.fetch_block_entry(),
            '?' if is_blankz(next) || (self.flow_level > 0 && is_flow_indicator(next)) => {
                self.fetch_key()
            }
            ':' if is_blankz(next)
                || (self.flow_level > 0 && (is_flow_indicator(next) || adjacent_value)) =>
            {
                self.fetch_value()
            }
            '*' => self.fetch_anchor(true),
            '&' => self.fetch_anchor(false),
            '!' => self.fetch_tag(),
            '|' | '>' if self.flow_level == 0 => self.fetch_block_scalar(c == '|'),
            '\'' | '"' => self.fetch_flow_scalar(c == '"'),
            _ if self.check_plain() => self.fetch_plain(),
            _ => self.error(format!("found character {c:?} that cannot start any token")),
        }
    }

    fn check_plain(&self) -> bool {
        let c = self.ch(0);
        if !is_blankz(c) && !is_indicator(c) {
            return true;
        }
        let next = self.ch(1);
        matches!(c, '-' | '?' | ':')
            && !is_blankz(next)
            && !(self.flow_level > 0 && is_flow_indicator(next))
    }

    // ---- whitespace and comments -------------------------------------------

    /// Whether the rest of the current line is only whitespace and an optional
    /// comment. Such a line may contain tabs anywhere.
    fn rest_of_line_is_blank(&self) -> bool {
        let mut k = 0;
        loop {
            match self.ch(k) {
                ' ' | '\t' => k += 1,
                '#' => return k > 0 || self.mark.column == 0 || is_blank(self.prev()),
                c => return is_breakz(c),
            }
        }
    }

    fn prev(&self) -> char {
        if self.mark.index == 0 {
            '\n'
        } else {
            self.buf[self.mark.index - 1]
        }
    }

    /// The current line's leading spaces, and whether a tab follows them before
    /// the current position.
    fn line_indentation(&self) -> (usize, bool) {
        let start = self.mark.index - self.mark.column;
        let line = &self.buf[start..self.mark.index];
        let spaces = line.iter().take_while(|&&c| c == ' ').count();
        let tabs = line[spaces..]
            .iter()
            .take_while(|&&c| is_blank(c))
            .any(|&c| c == '\t');
        (spaces, tabs)
    }

    fn scan_to_next_token(&mut self) -> Result<(), Error> {
        loop {
            // Tabs separate tokens anywhere; where they would stand in for
            // indentation, `fetch_next` and `roll_indent` refuse them.
            while is_blank(self.ch(0)) {
                self.forward();
            }
            if self.ch(0) == '#' {
                if !is_blankz(self.prev()) {
                    return self
                        .error("a comment must be separated from other tokens by whitespace");
                }
                while !is_breakz(self.ch(0)) {
                    self.forward();
                }
            }
            if is_break(self.ch(0)) {
                self.forward();
                if self.flow_level == 0 {
                    self.simple_key_allowed = true;
                }
            } else {
                return Ok(());
            }
        }
    }

    // ---- simple keys ---------------------------------------------------------

    fn stale_simple_keys(&mut self) -> Result<(), Error> {
        let mark = self.mark;
        for (level, key) in self.simple_keys.iter_mut().enumerate() {
            let in_flow_mapping = level > 0 && self.flow_mappings[level - 1];
            if key.possible
                && !in_flow_mapping
                && (key.mark.line != mark.line
                    || mark.index - key.mark.index > MAX_SIMPLE_KEY_LENGTH)
            {
                if key.required {
                    return Err(Error::syntax(
                        "could not find the expected ':' after this key",
                        key.mark,
                    ));
                }
                key.possible = false;
            }
        }
        Ok(())
    }

    fn save_simple_key(&mut self) -> Result<(), Error> {
        let required = self.flow_level == 0 && self.indent == self.col();
        if self.simple_key_allowed {
            self.remove_simple_key()?;
            let token_number = self.tokens_taken + self.tokens.len();
            *self.simple_keys.last_mut().unwrap() = SimpleKey {
                possible: true,
                required,
                token_number,
                mark: self.mark,
            };
        }
        Ok(())
    }

    fn remove_simple_key(&mut self) -> Result<(), Error> {
        let key = self.simple_keys.last_mut().unwrap();
        if key.possible && key.required {
            return Err(Error::syntax(
                "could not find the expected ':' after this key",
                key.mark,
            ));
        }
        key.possible = false;
        Ok(())
    }

    // ---- indentation ---------------------------------------------------------

    fn unwind_indent(&mut self, column: isize) {
        if self.flow_level > 0 {
            return;
        }
        while self.indent > column {
            self.indent = self.indents.pop().unwrap_or(-1);
            self.push(TokenKind::BlockEnd, self.mark);
        }
    }

    /// Opens a block collection at `column` if it is deeper than the current
    /// one, inserting its start token at `at` (a token number) or at the end.
    fn roll_indent(
        &mut self,
        column: isize,
        at: Option<usize>,
        kind: TokenKind,
        mark: Mark,
    ) -> Result<(), Error> {
        if self.flow_level > 0 || self.indent >= column {
            return Ok(());
        }
        // Only spaces may indent a block collection, including the compact
        // form after `-`, `?` or `:` on the same line.
        let before = self.buf[..mark.index]
            .iter()
            .rev()
            .take_while(|&&c| is_blank(c));
        if before.clone().any(|&c| c == '\t') {
            return Err(Error::syntax("tabs are not allowed as indentation", mark));
        }
        if self.document_start_line == Some(mark.line) {
            return Err(Error::syntax(
                "a block collection cannot start on the same line as '---'",
                mark,
            ));
        }
        self.indents.push(self.indent);
        self.indent = column;
        let token = Token { kind, start: mark };
        match at {
            Some(n) => self.tokens.insert(n - self.tokens_taken, token),
            None => self.tokens.push_back(token),
        }
        Ok(())
    }

    // ---- fetchers ------------------------------------------------------------

    fn fetch_stream_end(&mut self) -> Result<(), Error> {
        if self.mark.column != 0 {
            // Close the last line so BlockEnd marks are sensible.
            self.mark.column = 0;
            self.mark.line += 1;
        }
        self.unwind_indent(-1);
        self.remove_simple_key()?;
        if self.flow_level > 0 {
            return self.error("the stream ended inside a flow collection");
        }
        self.simple_key_allowed = false;
        self.push(TokenKind::StreamEnd, self.mark);
        self.done = true;
        Ok(())
    }

    fn fetch_directive(&mut self) -> Result<(), Error> {
        self.unwind_indent(-1);
        self.remove_simple_key()?;
        self.simple_key_allowed = false;
        let start = self.mark;
        let kind = self.scan_directive()?;
        self.push(kind, start);
        Ok(())
    }

    fn fetch_document_indicator(&mut self, start_marker: bool) -> Result<(), Error> {
        self.unwind_indent(-1);
        self.remove_simple_key()?;
        self.simple_key_allowed = false;
        let start = self.mark;
        self.forward_n(3);
        if start_marker {
            self.document_start_line = Some(start.line);
            self.push(TokenKind::DocumentStart, start);
        } else {
            // Only a comment may follow `...` on its line.
            while is_blank(self.ch(0)) {
                self.forward();
            }
            if !is_breakz(self.ch(0)) && self.ch(0) != '#' {
                return self.error("only a comment may follow '...' on its line");
            }
            self.push(TokenKind::DocumentEnd, start);
        }
        Ok(())
    }

    fn fetch_flow_collection_start(&mut self, kind: TokenKind) -> Result<(), Error> {
        self.save_simple_key()?;
        self.flow_level += 1;
        self.flow_mappings.push(kind == TokenKind::FlowMappingStart);
        self.simple_keys.push(SimpleKey::default());
        self.simple_key_allowed = true;
        let start = self.mark;
        self.forward();
        self.push(kind, start);
        Ok(())
    }

    fn fetch_flow_collection_end(&mut self, kind: TokenKind) -> Result<(), Error> {
        self.remove_simple_key()?;
        if self.flow_level == 0 {
            return self.error(format!("unexpected {:?}", self.ch(0)));
        }
        self.flow_level -= 1;
        self.flow_mappings.pop();
        self.simple_keys.pop();
        self.simple_key_allowed = false;
        let start = self.mark;
        self.forward();
        self.push(kind, start);
        self.json_like = true;
        Ok(())
    }

    fn fetch_flow_entry(&mut self) -> Result<(), Error> {
        self.simple_key_allowed = true;
        self.remove_simple_key()?;
        let start = self.mark;
        self.forward();
        self.push(TokenKind::FlowEntry, start);
        Ok(())
    }

    fn fetch_block_entry(&mut self) -> Result<(), Error> {
        if self.flow_level > 0 {
            return self.error("a block sequence entry is not allowed inside a flow collection");
        }
        if !self.simple_key_allowed {
            return self.error("a block sequence entry is not allowed in this context");
        }
        self.roll_indent(self.col(), None, TokenKind::BlockSequenceStart, self.mark)?;
        self.simple_key_allowed = true;
        self.remove_simple_key()?;
        let start = self.mark;
        self.forward();
        self.push(TokenKind::BlockEntry, start);
        Ok(())
    }

    fn fetch_key(&mut self) -> Result<(), Error> {
        if self.flow_level == 0 {
            if !self.simple_key_allowed {
                return self.error("a mapping key is not allowed in this context");
            }
            self.roll_indent(self.col(), None, TokenKind::BlockMappingStart, self.mark)?;
        }
        self.simple_key_allowed = self.flow_level == 0;
        self.remove_simple_key()?;
        let start = self.mark;
        self.forward();
        self.push(TokenKind::Key, start);
        Ok(())
    }

    fn fetch_value(&mut self) -> Result<(), Error> {
        let key = *self.simple_keys.last().unwrap();
        if key.possible {
            self.tokens.insert(
                key.token_number - self.tokens_taken,
                Token {
                    kind: TokenKind::Key,
                    start: key.mark,
                },
            );
            self.roll_indent(
                key.mark.column as isize,
                Some(key.token_number),
                TokenKind::BlockMappingStart,
                key.mark,
            )?;
            self.simple_keys.last_mut().unwrap().possible = false;
            self.simple_key_allowed = false;
        } else {
            if self.flow_level == 0 {
                // A `:` with no implicit key before it is the value half of an
                // explicit (`?`) entry or an empty key — never the end of
                // something on this line that could not be a key.
                if !self.simple_key_allowed || self.last_node_end_line == Some(self.mark.line) {
                    return self.error("a mapping value is not allowed in this context");
                }
                self.roll_indent(self.col(), None, TokenKind::BlockMappingStart, self.mark)?;
            }
            self.simple_key_allowed = self.flow_level == 0;
            self.remove_simple_key()?;
        }
        let start = self.mark;
        self.forward();
        self.push(TokenKind::Value, start);
        Ok(())
    }

    fn fetch_anchor(&mut self, alias: bool) -> Result<(), Error> {
        self.save_simple_key()?;
        self.simple_key_allowed = false;
        let start = self.mark;
        self.forward();
        let mut name = String::new();
        while !is_blankz(self.ch(0)) && !is_flow_indicator(self.ch(0)) {
            name.push(self.ch(0));
            self.forward();
        }
        if name.is_empty() {
            return Err(Error::syntax(
                if alias {
                    "an alias needs a name"
                } else {
                    "an anchor needs a name"
                },
                start,
            ));
        }
        self.push(
            if alias {
                TokenKind::Alias(name)
            } else {
                TokenKind::Anchor(name)
            },
            start,
        );
        Ok(())
    }

    fn fetch_tag(&mut self) -> Result<(), Error> {
        self.save_simple_key()?;
        self.simple_key_allowed = false;
        let start = self.mark;
        let (handle, suffix) = if self.ch(1) == '<' {
            self.forward_n(2);
            let suffix = self.scan_uri(true, start)?;
            if self.ch(0) != '>' {
                return self.error("a verbatim tag must end with '>'");
            }
            self.forward();
            if suffix.is_empty() {
                return Err(Error::syntax("a verbatim tag cannot be empty", start));
            }
            (String::new(), suffix)
        } else {
            // `!!x` and `!name!x` carry a named handle; `!x` and `!` the primary one.
            let mut k = 1;
            while is_word_char(self.ch(k)) {
                k += 1;
            }
            let handle = if self.ch(k) == '!' {
                let h: String = (0..=k).map(|i| self.ch(i)).collect();
                self.forward_n(k + 1);
                h
            } else {
                self.forward();
                "!".to_string()
            };
            let suffix = self.scan_uri(false, start)?;
            if suffix.is_empty() && handle != "!" {
                return Err(Error::syntax(
                    "a tag needs a suffix after its handle",
                    start,
                ));
            }
            (handle, suffix)
        };
        let c = self.ch(0);
        if !is_blankz(c) && !(self.flow_level > 0 && c == ',') {
            return self.error("a tag must be followed by whitespace");
        }
        self.push(TokenKind::Tag(handle, suffix), start);
        Ok(())
    }

    /// Scans URI characters, decoding `%XX` escapes. `verbatim` admits `!` and
    /// the flow indicators, which a tag shorthand's suffix may not contain.
    fn scan_uri(&mut self, verbatim: bool, start: Mark) -> Result<String, Error> {
        let mut bytes = Vec::new();
        loop {
            let c = self.ch(0);
            if !is_uri_char(c) || (!verbatim && (c == '!' || is_flow_indicator(c))) {
                break;
            }
            if c == '%' {
                let hex: String = [self.ch(1), self.ch(2)].iter().collect();
                let Ok(b) = u8::from_str_radix(&hex, 16) else {
                    return self.error("invalid %-escape in a tag");
                };
                bytes.push(b);
                self.forward_n(3);
            } else {
                let mut tmp = [0u8; 4];
                bytes.extend_from_slice(c.encode_utf8(&mut tmp).as_bytes());
                self.forward();
            }
        }
        String::from_utf8(bytes)
            .map_err(|_| Error::syntax("a tag's %-escapes are not UTF-8", start))
    }

    fn scan_directive(&mut self) -> Result<TokenKind, Error> {
        let start = self.mark;
        self.forward();
        let mut name = String::new();
        while !is_blankz(self.ch(0)) {
            name.push(self.ch(0));
            self.forward();
        }
        let kind = match name.as_str() {
            "YAML" => {
                self.skip_required_blanks()?;
                let major = self.scan_version_number()?;
                if self.ch(0) != '.' {
                    return self.error("expected a version like 1.2 after %YAML");
                }
                self.forward();
                let minor = self.scan_version_number()?;
                TokenKind::VersionDirective(major, minor)
            }
            "TAG" => {
                self.skip_required_blanks()?;
                if self.ch(0) != '!' {
                    return self.error("expected a tag handle after %TAG");
                }
                let mut handle = String::from("!");
                self.forward();
                while is_word_char(self.ch(0)) {
                    handle.push(self.ch(0));
                    self.forward();
                }
                if self.ch(0) == '!' {
                    handle.push('!');
                    self.forward();
                } else if handle != "!" {
                    return self.error("a named tag handle must end with '!'");
                }
                self.skip_required_blanks()?;
                let prefix = self.scan_uri(true, start)?;
                if prefix.is_empty() {
                    return self.error("expected a tag prefix after the handle");
                }
                TokenKind::TagDirective(handle, prefix)
            }
            "" => return Err(Error::syntax("a directive needs a name", start)),
            _ => {
                while !is_breakz(self.ch(0)) {
                    self.forward();
                }
                TokenKind::ReservedDirective
            }
        };
        while is_blank(self.ch(0)) {
            self.forward();
        }
        if self.ch(0) == '#' {
            if !is_blank(self.prev()) {
                return self.error("a comment must be separated from other tokens by whitespace");
            }
            while !is_breakz(self.ch(0)) {
                self.forward();
            }
        }
        if !is_breakz(self.ch(0)) {
            return self.error("unexpected content after a directive");
        }
        Ok(kind)
    }

    fn skip_required_blanks(&mut self) -> Result<(), Error> {
        if !is_blank(self.ch(0)) {
            return self.error("expected whitespace in a directive");
        }
        while is_blank(self.ch(0)) {
            self.forward();
        }
        Ok(())
    }

    fn scan_version_number(&mut self) -> Result<u32, Error> {
        let mut n: u32 = 0;
        let mut digits = 0;
        while let Some(d) = self.ch(0).to_digit(10) {
            n = n.saturating_mul(10).saturating_add(d);
            digits += 1;
            self.forward();
        }
        if digits == 0 {
            return self.error("expected a version number");
        }
        Ok(n)
    }

    // ---- block scalars ---------------------------------------------------------

    fn fetch_block_scalar(&mut self, literal: bool) -> Result<(), Error> {
        self.simple_key_allowed = true;
        self.remove_simple_key()?;
        let start = self.mark;
        let value = self.scan_block_scalar(literal)?;
        let style = if literal {
            ScalarStyle::Literal
        } else {
            ScalarStyle::Folded
        };
        self.push(TokenKind::Scalar(value, style), start);
        // A block scalar ends with its last line, so nothing after it shares one.
        self.last_node_end_line = None;
        Ok(())
    }

    fn scan_block_scalar(&mut self, literal: bool) -> Result<String, Error> {
        self.forward();
        // Header: chomping and indentation indicators, in either order.
        let mut chomping: i8 = 0; // -1 strip, 0 clip, 1 keep
        let mut increment: Option<usize> = None;
        for _ in 0..2 {
            match self.ch(0) {
                '+' | '-' if chomping == 0 => {
                    chomping = if self.ch(0) == '+' { 1 } else { -1 };
                    self.forward();
                }
                '1'..='9' if increment.is_none() => {
                    increment = self.ch(0).to_digit(10).map(|d| d as usize);
                    self.forward();
                }
                '0' => return self.error("a block scalar's indentation indicator cannot be 0"),
                _ => break,
            }
        }
        while is_blank(self.ch(0)) {
            self.forward();
        }
        if self.ch(0) == '#' {
            if !is_blank(self.prev()) {
                return self.error("a comment must be separated from other tokens by whitespace");
            }
            while !is_breakz(self.ch(0)) {
                self.forward();
            }
        }
        if !is_breakz(self.ch(0)) {
            return self.error("expected a comment or a line break after a block scalar header");
        }
        if is_break(self.ch(0)) {
            self.forward();
        }

        let parent = self.indent;
        let min_indent: usize = if parent < 0 { 0 } else { parent as usize + 1 };
        let mut breaks = String::new();
        let indent = match increment {
            Some(inc) => {
                let indent = if parent < 0 {
                    inc - 1
                } else {
                    parent as usize + inc
                };
                self.scan_block_scalar_breaks(indent, &mut breaks)?;
                indent
            }
            None => {
                // The first non-empty line sets the indentation; leading empty
                // lines may not be indented further than it.
                let mut max_blank = 0;
                loop {
                    while self.ch(0) == ' ' {
                        self.forward();
                    }
                    if self.mark.column > max_blank {
                        max_blank = self.mark.column;
                    }
                    if self.ch(0) == '\t' && self.mark.column < min_indent {
                        return self.error("tabs are not allowed as indentation");
                    }
                    if is_break(self.ch(0)) {
                        breaks.push('\n');
                        self.forward();
                    } else {
                        // A last line of spaces with no break still ends a line.
                        if self.ch(0) == '\0' && self.mark.column > 0 {
                            breaks.push('\n');
                        }
                        break;
                    }
                }
                let content = !is_breakz(self.ch(0));
                if content && self.mark.column < max_blank {
                    return self.error(
                        "a leading empty line in a block scalar is indented more than its content",
                    );
                }
                max_blank.max(min_indent)
            }
        };

        let mut out = String::new();
        let mut line_break = String::new();
        let mut leading_blank_prev;
        while self.mark.column == indent && self.ch(0) != '\0' {
            if indent == 0 && self.at_document_marker() {
                break;
            }
            out.push_str(&breaks);
            breaks.clear();
            leading_blank_prev = is_blank(self.ch(0));
            let mut only_spaces = true;
            while !is_breakz(self.ch(0)) {
                only_spaces &= self.ch(0) == ' ';
                out.push(self.ch(0));
                self.forward();
            }
            line_break.clear();
            // A last line of spaces with no break still ends a line.
            if is_break(self.ch(0)) || only_spaces {
                line_break.push('\n');
                self.forward();
            }
            self.scan_block_scalar_breaks(indent, &mut breaks)?;
            if self.mark.column == indent
                && self.ch(0) != '\0'
                && !(indent == 0 && self.at_document_marker())
            {
                if !literal && line_break == "\n" && !leading_blank_prev && !is_blank(self.ch(0)) {
                    if breaks.is_empty() {
                        out.push(' ');
                    }
                } else {
                    out.push_str(&line_break);
                }
            } else {
                break;
            }
        }
        if chomping >= 0 {
            out.push_str(&line_break);
        }
        if chomping > 0 {
            out.push_str(&breaks);
        }
        Ok(out)
    }

    /// Consumes empty lines (and indentation up to `indent`), collecting a line
    /// feed for each.
    fn scan_block_scalar_breaks(
        &mut self,
        indent: usize,
        breaks: &mut String,
    ) -> Result<(), Error> {
        loop {
            while self.mark.column < indent && self.ch(0) == ' ' {
                self.forward();
            }
            if self.mark.column < indent && self.ch(0) == '\t' && self.rest_of_line_is_blank() {
                while is_blank(self.ch(0)) {
                    self.forward();
                }
            }
            if is_break(self.ch(0)) {
                breaks.push('\n');
                self.forward();
            } else {
                return Ok(());
            }
        }
    }

    // ---- quoted scalars --------------------------------------------------------

    fn fetch_flow_scalar(&mut self, double: bool) -> Result<(), Error> {
        self.save_simple_key()?;
        self.simple_key_allowed = false;
        let start = self.mark;
        let value = self.scan_flow_scalar(double, start)?;
        let style = if double {
            ScalarStyle::DoubleQuoted
        } else {
            ScalarStyle::SingleQuoted
        };
        self.push(TokenKind::Scalar(value, style), start);
        self.json_like = true;
        Ok(())
    }

    fn scan_flow_scalar(&mut self, double: bool, start: Mark) -> Result<String, Error> {
        let quote = self.ch(0);
        self.forward();
        let mut out = String::new();
        loop {
            // Non-whitespace run.
            loop {
                let c = self.ch(0);
                if c == '\0' {
                    return Err(Error::syntax(
                        "the stream ended inside a quoted scalar",
                        start,
                    ));
                }
                if !double && c == '\'' && self.ch(1) == '\'' {
                    out.push('\'');
                    self.forward_n(2);
                } else if c == quote {
                    break;
                } else if double && c == '\\' {
                    if is_break(self.ch(1)) {
                        self.forward_n(2);
                        self.scan_flow_scalar_breaks(&mut out, start)?;
                        // An escaped line break joins without a space; its
                        // following indentation is not content.
                        continue;
                    }
                    self.scan_escape(&mut out)?;
                } else if is_blank(c) || is_break(c) {
                    break;
                } else {
                    out.push(c);
                    self.forward();
                }
            }
            if self.ch(0) == quote {
                self.forward();
                return Ok(out);
            }
            // Whitespace run, possibly folding line breaks.
            let mut whitespace = String::new();
            while is_blank(self.ch(0)) {
                whitespace.push(self.ch(0));
                self.forward();
            }
            if is_break(self.ch(0)) {
                self.forward();
                let mut breaks = String::new();
                self.scan_flow_scalar_breaks(&mut breaks, start)?;
                if breaks.is_empty() {
                    out.push(' ');
                } else {
                    out.push_str(&breaks);
                }
            } else {
                out.push_str(&whitespace);
            }
        }
    }

    fn scan_flow_scalar_breaks(&mut self, out: &mut String, start: Mark) -> Result<(), Error> {
        loop {
            if self.at_document_marker() {
                return Err(Error::syntax(
                    "a document marker cannot appear inside a quoted scalar",
                    start,
                ));
            }
            while is_blank(self.ch(0)) {
                self.forward();
            }
            if is_break(self.ch(0)) {
                out.push('\n');
                self.forward();
                continue;
            }
            if self.ch(0) == '\0' {
                return Err(Error::syntax(
                    "the stream ended inside a quoted scalar",
                    start,
                ));
            }
            if self.flow_level == 0 && (self.line_indentation().0 as isize) <= self.indent {
                return self.error("a quoted scalar's continuation line is not indented enough");
            }
            return Ok(());
        }
    }

    fn scan_escape(&mut self, out: &mut String) -> Result<(), Error> {
        let c = self.ch(1);
        let simple = match c {
            '0' => Some('\0'),
            'a' => Some('\u{7}'),
            'b' => Some('\u{8}'),
            't' | '\t' => Some('\t'),
            'n' => Some('\n'),
            'v' => Some('\u{b}'),
            'f' => Some('\u{c}'),
            'r' => Some('\r'),
            'e' => Some('\u{1b}'),
            ' ' => Some(' '),
            '"' => Some('"'),
            '/' => Some('/'),
            '\\' => Some('\\'),
            'N' => Some('\u{85}'),
            '_' => Some('\u{a0}'),
            'L' => Some('\u{2028}'),
            'P' => Some('\u{2029}'),
            _ => None,
        };
        if let Some(ch) = simple {
            out.push(ch);
            self.forward_n(2);
            return Ok(());
        }
        let len = match c {
            'x' => 2,
            'u' => 4,
            'U' => 8,
            _ => return self.error(format!("unknown escape sequence \\{c}")),
        };
        let hex: String = (0..len).map(|i| self.ch(2 + i)).collect();
        let code = u32::from_str_radix(&hex, 16)
            .ok()
            .filter(|_| hex.chars().all(|h| h.is_ascii_hexdigit()));
        let Some(ch) = code.and_then(char::from_u32) else {
            return self.error(format!("invalid escape sequence \\{c}{hex}"));
        };
        out.push(ch);
        self.forward_n(2 + len);
        Ok(())
    }

    // ---- plain scalars ---------------------------------------------------------

    fn fetch_plain(&mut self) -> Result<(), Error> {
        self.save_simple_key()?;
        self.simple_key_allowed = false;
        let start = self.mark;
        let (value, end) = self.scan_plain()?;
        self.push(TokenKind::Scalar(value, ScalarStyle::Plain), start);
        // The scan went on through trailing whitespace, line breaks included.
        self.last_node_end_line = Some(end.line);
        Ok(())
    }

    fn scan_plain(&mut self) -> Result<(String, Mark), Error> {
        let mut out = String::new();
        let mut end = self.mark;
        let mut spaces = String::new();
        let min_indent = self.indent + 1;
        loop {
            if self.ch(0) == '#' {
                break;
            }
            let mut length = 0;
            loop {
                let c = self.ch(length);
                if is_blankz(c) {
                    break;
                }
                if c == ':' {
                    let next = self.ch(length + 1);
                    if is_blankz(next) || (self.flow_level > 0 && is_flow_indicator(next)) {
                        break;
                    }
                }
                if self.flow_level > 0 && is_flow_indicator(c) {
                    break;
                }
                length += 1;
            }
            if length == 0 {
                break;
            }
            self.simple_key_allowed = false;
            out.push_str(&spaces);
            for _ in 0..length {
                out.push(self.ch(0));
                self.forward();
            }
            end = self.mark;
            spaces.clear();
            if !self.scan_plain_spaces(&mut spaces)?
                || self.ch(0) == '#'
                || (self.flow_level == 0 && self.col() < min_indent)
            {
                break;
            }
        }
        Ok((out, end))
    }

    /// Consumes whitespace after a plain scalar's word, folding line breaks.
    /// Returns whether the scalar may continue.
    fn scan_plain_spaces(&mut self, out: &mut String) -> Result<bool, Error> {
        let mut whitespace = String::new();
        while is_blank(self.ch(0)) {
            whitespace.push(self.ch(0));
            self.forward();
        }
        if !is_break(self.ch(0)) {
            out.push_str(&whitespace);
            return Ok(!whitespace.is_empty());
        }
        self.forward();
        self.simple_key_allowed = true;
        if self.at_document_marker() {
            return Ok(false);
        }
        let mut breaks = String::new();
        loop {
            match self.ch(0) {
                ' ' => self.forward(),
                '\t' => {
                    // A tab may follow the indentation of a continuation line,
                    // but cannot be the indentation itself.
                    if self.flow_level == 0
                        && self.col() <= self.indent
                        && !self.rest_of_line_is_blank()
                    {
                        return Ok(false);
                    }
                    self.forward()
                }
                '\n' => {
                    breaks.push('\n');
                    self.forward();
                    if self.at_document_marker() {
                        return Ok(false);
                    }
                }
                _ => break,
            }
        }
        if breaks.is_empty() {
            out.push(' ');
        } else {
            out.push_str(&breaks);
        }
        Ok(true)
    }
}
