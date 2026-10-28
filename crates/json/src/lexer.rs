//! Tokens of JSONC and JSON5, each with the byte offset it starts at.
//!
//! Whitespace and comments are skipped between tokens.

use std::borrow::Cow;

use crate::{Dialect, Error, ErrorKind, Text};

#[derive(Debug, PartialEq)]
pub(crate) enum Kind<'a> {
    BeginObject,
    EndObject,
    BeginArray,
    EndArray,
    Colon,
    Comma,
    String(Text<'a>),
    Number(f64),
    /// A bare word: `true`, `false` and `null` in both dialects, and in JSON5
    /// any identifier name, which is a value only as one of those three or as
    /// `Infinity` or `NaN`.
    /// `escaped` says a `\u` escape spelled part of it, which keeps it a name:
    /// `true` is the key `true`, never the value.
    Name {
        text: Text<'a>,
        escaped: bool,
    },
    End,
}

#[derive(Debug)]
pub(crate) struct Token<'a> {
    pub kind: Kind<'a>,
    pub start: usize,
}

pub(crate) struct Lexer<'a> {
    text: &'a str,
    bytes: &'a [u8],
    pos: usize,
    dialect: Dialect,
}

type Lexed<T> = Result<T, Error>;

impl<'a> Lexer<'a> {
    pub fn new(text: &'a str, dialect: Dialect) -> Lexer<'a> {
        // A byte order mark is ignored at the start, as RFC 8259 §8.1 permits;
        // JSON5 also reads it as whitespace anywhere.
        let pos = if text.starts_with('\u{FEFF}') { 3 } else { 0 };
        Lexer {
            text,
            bytes: text.as_bytes(),
            pos,
            dialect,
        }
    }

    pub fn error(&self, message: impl Into<String>, offset: usize) -> Error {
        Error::new(ErrorKind::Syntax, message, self.text, offset, self.dialect)
    }

    fn json5(&self) -> bool {
        self.dialect == Dialect::Json5
    }

    fn peek_char(&self) -> Option<char> {
        self.text[self.pos..].chars().next()
    }

    fn char_at(&self, at: usize) -> Option<char> {
        self.text.get(at..).and_then(|rest| rest.chars().next())
    }

    /// Describes the character at `at` for an error message.
    fn describe(&self, at: usize) -> String {
        match self.char_at(at) {
            None => "the end of the text".into(),
            Some(c) if c.is_control() || is_json5_space(c) => {
                format!("U+{:04X}", u32::from(c))
            }
            Some(c) => format!("'{c}'"),
        }
    }

    fn unexpected(&self, at: usize) -> Error {
        if at >= self.bytes.len() {
            self.error("unexpected end of the text", at)
        } else {
            self.error(format!("unexpected {}", self.describe(at)), at)
        }
    }

    /// Skips whitespace and comments.
    fn skip_trivia(&mut self) -> Lexed<()> {
        loop {
            let Some(&b) = self.bytes.get(self.pos) else {
                return Ok(());
            };
            match b {
                b' ' | b'\t' | b'\n' | b'\r' => self.pos += 1,
                b'/' => match self.bytes.get(self.pos + 1) {
                    Some(b'/') => {
                        self.pos += 2;
                        while let Some(c) = self.peek_char() {
                            if self.ends_comment_line(c) {
                                break;
                            }
                            self.pos += c.len_utf8();
                        }
                    }
                    Some(b'*') => {
                        let start = self.pos;
                        match self.text[self.pos + 2..].find("*/") {
                            Some(at) => self.pos += 2 + at + 2,
                            None => return Err(self.error("a comment is not closed", start)),
                        }
                    }
                    _ => return Ok(()),
                },
                0x0b | 0x0c if self.json5() => self.pos += 1,
                _ if b >= 0x80 && self.json5() => match self.peek_char() {
                    Some(c) if is_json5_space(c) => self.pos += c.len_utf8(),
                    _ => return Ok(()),
                },
                _ => return Ok(()),
            }
        }
    }

    fn ends_comment_line(&self, c: char) -> bool {
        matches!(c, '\n' | '\r') || (self.json5() && matches!(c, '\u{2028}' | '\u{2029}'))
    }

    pub fn next(&mut self) -> Lexed<Token<'a>> {
        self.skip_trivia()?;
        let start = self.pos;
        let Some(&b) = self.bytes.get(start) else {
            return Ok(Token {
                kind: Kind::End,
                start,
            });
        };
        let kind = match b {
            b'{' => self.punct(Kind::BeginObject),
            b'}' => self.punct(Kind::EndObject),
            b'[' => self.punct(Kind::BeginArray),
            b']' => self.punct(Kind::EndArray),
            b':' => self.punct(Kind::Colon),
            b',' => self.punct(Kind::Comma),
            b'"' => Kind::String(self.string(b'"')?),
            b'\'' if self.json5() => Kind::String(self.string(b'\'')?),
            b'-' | b'0'..=b'9' => Kind::Number(self.number()?),
            b'+' | b'.' if self.json5() => Kind::Number(self.number()?),
            b'/' => return Err(self.unexpected(start)),
            // A bare `Infinity` or `NaN` is a name here, since either may be a
            // key; the parser reads it as a number where a value is expected.
            _ if self.json5() => self.identifier()?,
            b'a'..=b'z' | b'A'..=b'Z' => self.word(),
            _ => return Err(self.unexpected(start)),
        };
        Ok(Token { kind, start })
    }

    fn punct(&mut self, kind: Kind<'a>) -> Kind<'a> {
        self.pos += 1;
        kind
    }

    /// A JSONC word. Only `true`, `false` and `null` mean anything; the parser
    /// names what was expected for any other, which reads better than an
    /// error at its first letter.
    fn word(&mut self) -> Kind<'a> {
        let start = self.pos;
        while self
            .bytes
            .get(self.pos)
            .is_some_and(u8::is_ascii_alphanumeric)
        {
            self.pos += 1;
        }
        Kind::Name {
            text: Text::Str(Cow::Borrowed(&self.text[start..self.pos])),
            escaped: false,
        }
    }

    // ---- strings ------------------------------------------------------------

    fn string(&mut self, quote: u8) -> Lexed<Text<'a>> {
        let start = self.pos;
        self.pos += 1;
        let body = self.pos;
        // The common case, no escape, borrows the input.
        loop {
            let Some(&b) = self.bytes.get(self.pos) else {
                return Err(self.error("a string is not closed", start));
            };
            if b == quote {
                let text = &self.text[body..self.pos];
                self.pos += 1;
                return Ok(Text::Str(Cow::Borrowed(text)));
            }
            if b == b'\\' {
                break;
            }
            self.string_char(b)?;
        }
        let mut out = Builder::new(&self.text[body..self.pos]);
        loop {
            let Some(&b) = self.bytes.get(self.pos) else {
                return Err(self.error("a string is not closed", start));
            };
            if b == quote {
                self.pos += 1;
                return Ok(out.finish());
            }
            if b == b'\\' {
                self.escape(&mut out)?;
                continue;
            }
            let from = self.pos;
            self.string_char(b)?;
            out.push_str(&self.text[from..self.pos]);
        }
    }

    /// Steps over one unescaped character of a string, refusing what the
    /// dialect requires to be escaped.
    fn string_char(&mut self, b: u8) -> Lexed<()> {
        match b {
            b'\n' | b'\r' => Err(self.error("a line break in a string must be escaped", self.pos)),
            0x00..=0x1f if !self.json5() => Err(self.error(
                format!("{} in a string must be escaped", self.describe(self.pos)),
                self.pos,
            )),
            0x80.. => {
                let c = self.peek_char().expect("a lead byte starts a char");
                self.pos += c.len_utf8();
                Ok(())
            }
            _ => {
                self.pos += 1;
                Ok(())
            }
        }
    }

    fn escape(&mut self, out: &mut Builder) -> Lexed<()> {
        let at = self.pos;
        self.pos += 1;
        let Some(c) = self.peek_char() else {
            return Err(self.error("a string is not closed", at));
        };
        self.pos += c.len_utf8();
        let invalid = |lexer: &Self| lexer.error(format!("invalid escape '\\{c}'"), at);
        match c {
            '"' | '\\' | '/' => out.push_char(c),
            'b' => out.push_char('\u{8}'),
            'f' => out.push_char('\u{c}'),
            'n' => out.push_char('\n'),
            'r' => out.push_char('\r'),
            't' => out.push_char('\t'),
            'u' => {
                let unit = self.hex(4, at)?;
                out.push_unit(unit as u16);
            }
            _ if !self.json5() => return Err(invalid(self)),
            'v' => out.push_char('\u{b}'),
            '0' => {
                if self.bytes.get(self.pos).is_some_and(u8::is_ascii_digit) {
                    return Err(invalid(self));
                }
                out.push_char('\0');
            }
            '1'..='9' => return Err(invalid(self)),
            'x' => {
                let unit = self.hex(2, at)?;
                out.push_unit(unit as u16);
            }
            // A line continuation: the backslash and the line break vanish.
            '\n' | '\u{2028}' | '\u{2029}' => {}
            '\r' => {
                if self.bytes.get(self.pos) == Some(&b'\n') {
                    self.pos += 1;
                }
            }
            _ => out.push_char(c),
        }
        Ok(())
    }

    /// Exactly `digits` hex digits, as a number.
    fn hex(&mut self, digits: usize, escape: usize) -> Lexed<u32> {
        let mut value = 0u32;
        for _ in 0..digits {
            let digit = self
                .bytes
                .get(self.pos)
                .and_then(|b| char::from(*b).to_digit(16))
                .ok_or_else(|| {
                    self.error(format!("an escape needs {digits} hex digits"), escape)
                })?;
            value = value * 16 + digit;
            self.pos += 1;
        }
        Ok(value)
    }

    // ---- numbers ------------------------------------------------------------

    fn number(&mut self) -> Lexed<f64> {
        let start = self.pos;
        let value = if self.json5() {
            self.json5_number(start)?
        } else {
            self.json_number(start)?
        };
        // `01`, `1.2.3`, `0x` and `1true` are one bad number, not two tokens.
        if let Some(c) = self.peek_char()
            && (c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '$')
        {
            return Err(self.error("invalid number", start));
        }
        Ok(value)
    }

    fn digits(&mut self) -> usize {
        let from = self.pos;
        while self.bytes.get(self.pos).is_some_and(u8::is_ascii_digit) {
            self.pos += 1;
        }
        self.pos - from
    }

    /// RFC 8259's `number`.
    fn json_number(&mut self, start: usize) -> Lexed<f64> {
        if self.bytes[self.pos] == b'-' {
            self.pos += 1;
        }
        match self.bytes.get(self.pos) {
            Some(b'0') => self.pos += 1,
            Some(b'1'..=b'9') => {
                self.digits();
            }
            _ => return Err(self.error("invalid number", start)),
        }
        if self.bytes.get(self.pos) == Some(&b'.') {
            self.pos += 1;
            if self.digits() == 0 {
                return Err(self.error("invalid number", start));
            }
        }
        self.exponent(start)?;
        Ok(parse_decimal(&self.text[start..self.pos]))
    }

    fn exponent(&mut self, start: usize) -> Lexed<()> {
        if matches!(self.bytes.get(self.pos), Some(b'e' | b'E')) {
            self.pos += 1;
            if matches!(self.bytes.get(self.pos), Some(b'+' | b'-')) {
                self.pos += 1;
            }
            if self.digits() == 0 {
                return Err(self.error("invalid number", start));
            }
        }
        Ok(())
    }

    /// JSON5's `JSON5Number`: an ECMAScript numeric literal, `Infinity` or
    /// `NaN`, with an optional sign.
    fn json5_number(&mut self, start: usize) -> Lexed<f64> {
        let negative = match self.bytes[self.pos] {
            b'-' => {
                self.pos += 1;
                true
            }
            b'+' => {
                self.pos += 1;
                false
            }
            _ => false,
        };
        let sign = if negative { -1.0 } else { 1.0 };
        let rest = &self.text[self.pos..];
        if rest.starts_with("Infinity") {
            self.pos += "Infinity".len();
            return Ok(sign * f64::INFINITY);
        }
        if rest.starts_with("NaN") {
            self.pos += "NaN".len();
            return Ok(f64::NAN);
        }
        if rest.starts_with("0x") || rest.starts_with("0X") {
            self.pos += 2;
            let from = self.pos;
            while self.bytes.get(self.pos).is_some_and(u8::is_ascii_hexdigit) {
                self.pos += 1;
            }
            if self.pos == from {
                return Err(self.error("invalid number", start));
            }
            return Ok(sign * crate::number::hex_to_f64(&self.bytes[from..self.pos]));
        }
        let integer_start = self.pos;
        let integer = self.digits();
        if integer > 1 && self.bytes[integer_start] == b'0' {
            return Err(self.error("invalid number", start));
        }
        let mut fraction = 0;
        if self.bytes.get(self.pos) == Some(&b'.') {
            self.pos += 1;
            fraction = self.digits();
        }
        if integer == 0 && fraction == 0 {
            return Err(self.error("invalid number", start));
        }
        self.exponent(start)?;
        // Rust reads `5.`, `.5` and `5.e3` as ECMAScript does, and rounds
        // correctly; only a leading `+` needs removing.
        Ok(sign * parse_decimal(&self.text[integer_start..self.pos]))
    }

    // ---- identifiers --------------------------------------------------------

    /// An ECMAScript `IdentifierName`, with `\uXXXX` escapes.
    fn identifier(&mut self) -> Lexed<Kind<'a>> {
        let start = self.pos;
        let mut escaped = false;
        let mut out: Option<Builder> = None;
        loop {
            let at = self.pos;
            let first = at == start;
            let (c, from_escape) = match self.peek_char() {
                Some('\\') => {
                    if self.bytes.get(at + 1) != Some(&b'u') {
                        return Err(self.error("invalid escape in a property name", at));
                    }
                    self.pos += 2;
                    let unit = self.hex(4, at)?;
                    match char::from_u32(unit) {
                        Some(c) if is_identifier_char(c, first) => (c, true),
                        _ => return Err(self.error("invalid escape in a property name", at)),
                    }
                }
                Some(c) if is_identifier_char(c, first) => {
                    self.pos += c.len_utf8();
                    (c, false)
                }
                _ if first => return Err(self.unexpected(at)),
                _ => break,
            };
            if from_escape && out.is_none() {
                out = Some(Builder::new(&self.text[start..at]));
            }
            escaped |= from_escape;
            if let Some(out) = &mut out {
                out.push_char(c);
            }
        }
        let text = match out {
            Some(out) => out.finish(),
            None => Text::Str(Cow::Borrowed(&self.text[start..self.pos])),
        };
        Ok(Kind::Name { text, escaped })
    }
}

fn parse_decimal(text: &str) -> f64 {
    text.parse()
        .expect("the lexer passes only ECMAScript decimal literals")
}

/// JSON5's whitespace beyond JSON's four: VT, FF, NBSP, the line and paragraph
/// separators, the byte order mark, and the Unicode `Zs` category.
fn is_json5_space(c: char) -> bool {
    matches!(
        c,
        '\u{b}' | '\u{c}' | '\u{a0}' | '\u{1680}' | '\u{2000}'
            ..='\u{200a}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202f}'
                | '\u{205f}'
                | '\u{3000}'
                | '\u{feff}'
    )
}

fn is_identifier_char(c: char, first: bool) -> bool {
    match c {
        '$' | '_' => true,
        'a'..='z' | 'A'..='Z' => true,
        '0'..='9' => !first,
        '\u{200c}' | '\u{200d}' => !first,
        _ if c.is_ascii() => false,
        _ if first => unicode_ident::is_xid_start(c),
        _ => unicode_ident::is_xid_continue(c),
    }
}

/// Builds a string that needed escapes, switching to UTF-16 only when an
/// escape spells a lone surrogate.
struct Builder {
    text: String,
    wide: Option<Vec<u16>>,
    /// A high surrogate waiting to see whether a low one follows.
    high: Option<u16>,
}

impl Builder {
    fn new(prefix: &str) -> Builder {
        Builder {
            text: prefix.to_owned(),
            wide: None,
            high: None,
        }
    }

    fn flush_high(&mut self) {
        if let Some(high) = self.high.take() {
            self.widen().push(high);
        }
    }

    fn widen(&mut self) -> &mut Vec<u16> {
        let text = &self.text;
        self.wide
            .get_or_insert_with(|| text.encode_utf16().collect())
    }

    fn push_char(&mut self, c: char) {
        self.flush_high();
        match &mut self.wide {
            Some(wide) => wide.extend(c.encode_utf16(&mut [0; 2]).iter()),
            None => self.text.push(c),
        }
    }

    fn push_str(&mut self, s: &str) {
        self.flush_high();
        match &mut self.wide {
            Some(wide) => wide.extend(s.encode_utf16()),
            None => self.text.push_str(s),
        }
    }

    fn push_unit(&mut self, unit: u16) {
        match unit {
            0xd800..=0xdbff => {
                self.flush_high();
                self.high = Some(unit);
            }
            0xdc00..=0xdfff => match self.high.take() {
                Some(high) => {
                    let c = char::decode_utf16([high, unit])
                        .next()
                        .and_then(Result::ok)
                        .expect("a high then a low surrogate is a char");
                    self.push_char(c);
                }
                None => self.widen().push(unit),
            },
            _ => self.push_char(char::from_u32(u32::from(unit)).expect("not a surrogate")),
        }
    }

    fn finish(mut self) -> Text<'static> {
        self.flush_high();
        match self.wide {
            Some(wide) => Text::Utf16(wide),
            None => Text::Str(Cow::Owned(self.text)),
        }
    }
}
