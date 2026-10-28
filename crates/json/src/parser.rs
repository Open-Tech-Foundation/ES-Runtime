//! The grammar, reported to a [`Sink`]. Punctuation is read here a byte at a
//! time; the lexer scans scalars.

use crate::lexer::Lexer;
use crate::{Dialect, Error, ErrorKind, Failure, MAX_DEPTH, Sink, Text};

/// Reads `text` as `dialect` and reports each value to `sink`.
pub fn parse<S: Sink>(text: &str, dialect: Dialect, sink: &mut S) -> Result<(), Failure<S::Abort>> {
    let mut parser = Parser {
        lexer: Lexer::new(text, dialect),
        text,
        dialect,
    };
    parser.value(sink, 0)?;
    if parser.lexer.peek()?.is_some() {
        let at = parser.lexer.pos();
        return Err(parser
            .lexer
            .error("unexpected text after the value", at)
            .into());
    }
    Ok(())
}

/// Whether `text` is `dialect`, without building anything.
pub fn validate(text: &str, dialect: Dialect) -> Result<(), Error> {
    match parse(text, dialect, &mut Discard) {
        Ok(()) => Ok(()),
        Err(Failure::Error(error)) => Err(*error),
        Err(Failure::Abort(never)) => match never {},
    }
}

struct Parser<'a> {
    lexer: Lexer<'a>,
    text: &'a str,
    dialect: Dialect,
}

type Parsed<A> = Result<(), Failure<A>>;

/// Whether `b` can start a bare word: a JSONC literal, or a JSON5 identifier.
fn starts_word(b: u8, json5: bool) -> bool {
    b.is_ascii_alphabetic() || (json5 && matches!(b, b'$' | b'_' | b'\\' | 0x80..))
}

impl<'a> Parser<'a> {
    fn value<S: Sink>(&mut self, sink: &mut S, depth: usize) -> Parsed<S::Abort> {
        let abort = Failure::Abort;
        let json5 = self.lexer.json5();
        let b = self.next_byte("a value")?;
        let at = self.lexer.pos();
        match b {
            b'{' => self.object(sink, depth + 1),
            b'[' => self.array(sink, depth + 1),
            b'"' => {
                let text = self.lexer.string(b'"')?;
                sink.string(text).map_err(abort)
            }
            b'\'' if json5 => {
                let text = self.lexer.string(b'\'')?;
                sink.string(text).map_err(abort)
            }
            b'-' | b'0'..=b'9' => {
                let n = self.lexer.number()?;
                sink.number(n).map_err(abort)
            }
            b'+' | b'.' if json5 => {
                let n = self.lexer.number()?;
                sink.number(n).map_err(abort)
            }
            _ if starts_word(b, json5) => {
                let (word, escaped) = self.lexer.word()?;
                match (&word, escaped) {
                    (Text::Str(word), false) => match &**word {
                        "true" => sink.bool(true).map_err(abort),
                        "false" => sink.bool(false).map_err(abort),
                        "null" => sink.null().map_err(abort),
                        "Infinity" if json5 => sink.number(f64::INFINITY).map_err(abort),
                        "NaN" if json5 => sink.number(f64::NAN).map_err(abort),
                        _ => Err(self.expected("a value", at)),
                    },
                    _ => Err(self.expected("a value", at)),
                }
            }
            _ => Err(self.expected("a value", at)),
        }
    }

    fn expected<A>(&self, what: &str, at: usize) -> Failure<A> {
        let found = match self.text[at..].chars().next() {
            None => "the end of the text".to_owned(),
            Some(c) if c.is_control() => format!("U+{:04X}", u32::from(c)),
            Some(c) => format!("'{}'", self.token_text(at, c)),
        };
        self.lexer
            .error(format!("expected {what}, found {found}"), at)
            .into()
    }

    /// A word for an error message, or else the one character.
    fn token_text(&self, at: usize, first: char) -> &'a str {
        let rest = &self.text[at..];
        if first.is_ascii_alphabetic() {
            let len = rest.bytes().take_while(u8::is_ascii_alphanumeric).count();
            &rest[..len.min(20)]
        } else {
            &rest[..first.len_utf8()]
        }
    }

    fn enter<A>(&self, depth: usize) -> Result<(), Failure<A>> {
        if depth > MAX_DEPTH {
            return Err(Error::new(
                ErrorKind::Depth,
                format!("the text nests more than {MAX_DEPTH} levels deep"),
                self.text,
                self.lexer.pos(),
                self.dialect,
            )
            .into());
        }
        Ok(())
    }

    /// The byte the next token starts with; at the end of the text, an error
    /// naming what was expected there.
    #[inline]
    fn next_byte<A>(&mut self, what: &str) -> Result<u8, Failure<A>> {
        match self.lexer.peek()? {
            Some(b) => Ok(b),
            None if what == "a value" => {
                let at = self.lexer.pos();
                Err(self.lexer.error("unexpected end of the text", at).into())
            }
            None => Err(self.expected(what, self.lexer.pos())),
        }
    }

    fn object<S: Sink>(&mut self, sink: &mut S, depth: usize) -> Parsed<S::Abort> {
        self.enter(depth)?;
        self.lexer.bump();
        sink.begin_object().map_err(Failure::Abort)?;
        if self.lexer.peek()? == Some(b'}') {
            self.lexer.bump();
            return sink.end_object().map_err(Failure::Abort);
        }
        let json5 = self.lexer.json5();
        loop {
            let b = self.next_byte("a property name")?;
            let at = self.lexer.pos();
            let key = match b {
                b'"' => self.lexer.string(b'"')?,
                b'\'' if json5 => self.lexer.string(b'\'')?,
                _ if json5 && starts_word(b, true) => self.lexer.word()?.0,
                _ if b.is_ascii_alphabetic() => {
                    return Err(self
                        .lexer
                        .error("a property name must be a double-quoted string", at)
                        .into());
                }
                _ => return Err(self.expected("a property name", at)),
            };
            sink.key(key).map_err(Failure::Abort)?;
            if self.next_byte("':' after the property name")? != b':' {
                return Err(self.expected("':' after the property name", self.lexer.pos()));
            }
            self.lexer.bump();
            self.value(sink, depth)?;
            match self.next_byte("',' or '}'")? {
                b'}' => {
                    self.lexer.bump();
                    break;
                }
                b',' => self.lexer.bump(),
                _ => return Err(self.expected("',' or '}'", self.lexer.pos())),
            }
            // One trailing comma may close the object.
            if self.lexer.peek()? == Some(b'}') {
                self.lexer.bump();
                break;
            }
        }
        sink.end_object().map_err(Failure::Abort)
    }

    fn array<S: Sink>(&mut self, sink: &mut S, depth: usize) -> Parsed<S::Abort> {
        self.enter(depth)?;
        self.lexer.bump();
        sink.begin_array().map_err(Failure::Abort)?;
        if self.lexer.peek()? == Some(b']') {
            self.lexer.bump();
            return sink.end_array().map_err(Failure::Abort);
        }
        loop {
            self.value(sink, depth)?;
            match self.next_byte("',' or ']'")? {
                b']' => {
                    self.lexer.bump();
                    break;
                }
                b',' => self.lexer.bump(),
                _ => return Err(self.expected("',' or ']'", self.lexer.pos())),
            }
            // One trailing comma may close the array.
            if self.lexer.peek()? == Some(b']') {
                self.lexer.bump();
                break;
            }
        }
        sink.end_array().map_err(Failure::Abort)
    }
}

/// A sink that keeps nothing, for [`validate`].
struct Discard;

impl Sink for Discard {
    type Abort = std::convert::Infallible;
    fn null(&mut self) -> Result<(), Self::Abort> {
        Ok(())
    }
    fn bool(&mut self, _: bool) -> Result<(), Self::Abort> {
        Ok(())
    }
    fn number(&mut self, _: f64) -> Result<(), Self::Abort> {
        Ok(())
    }
    fn string(&mut self, _: Text<'_>) -> Result<(), Self::Abort> {
        Ok(())
    }
    fn begin_array(&mut self) -> Result<(), Self::Abort> {
        Ok(())
    }
    fn end_array(&mut self) -> Result<(), Self::Abort> {
        Ok(())
    }
    fn begin_object(&mut self) -> Result<(), Self::Abort> {
        Ok(())
    }
    fn key(&mut self, _: Text<'_>) -> Result<(), Self::Abort> {
        Ok(())
    }
    fn end_object(&mut self) -> Result<(), Self::Abort> {
        Ok(())
    }
}
