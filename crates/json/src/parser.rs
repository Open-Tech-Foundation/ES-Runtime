//! The grammar over the lexer's tokens, reported to a [`Sink`].

use crate::lexer::{Kind, Lexer, Token};
use crate::{Dialect, Error, ErrorKind, Failure, MAX_DEPTH, Sink, Text};

/// Reads `text` as `dialect` and reports each value to `sink`.
pub fn parse<S: Sink>(text: &str, dialect: Dialect, sink: &mut S) -> Result<(), Failure<S::Abort>> {
    let mut parser = Parser {
        lexer: Lexer::new(text, dialect),
        text,
        dialect,
    };
    let first = parser.lexer.next()?;
    parser.value(first, sink, 0)?;
    let after = parser.lexer.next()?;
    if after.kind != Kind::End {
        return Err(parser
            .lexer
            .error("unexpected text after the value", after.start)
            .into());
    }
    Ok(())
}

/// Whether `text` is `dialect`, without building anything.
pub fn validate(text: &str, dialect: Dialect) -> Result<(), Error> {
    match parse(text, dialect, &mut Discard) {
        Ok(()) => Ok(()),
        Err(Failure::Error(error)) => Err(error),
        Err(Failure::Abort(never)) => match never {},
    }
}

struct Parser<'a> {
    lexer: Lexer<'a>,
    text: &'a str,
    dialect: Dialect,
}

type Parsed<A> = Result<(), Failure<A>>;

impl<'a> Parser<'a> {
    fn value<S: Sink>(&mut self, token: Token<'a>, sink: &mut S, depth: usize) -> Parsed<S::Abort> {
        let abort = Failure::Abort;
        match token.kind {
            Kind::BeginObject => self.object(token.start, sink, depth + 1),
            Kind::BeginArray => self.array(token.start, sink, depth + 1),
            Kind::String(text) => sink.string(text).map_err(abort),
            Kind::Number(n) => sink.number(n).map_err(abort),
            Kind::Name {
                text: Text::Str(word),
                escaped: false,
            } => match &*word {
                "true" => sink.bool(true).map_err(abort),
                "false" => sink.bool(false).map_err(abort),
                "null" => sink.null().map_err(abort),
                "Infinity" if self.dialect == Dialect::Json5 => {
                    sink.number(f64::INFINITY).map_err(abort)
                }
                "NaN" if self.dialect == Dialect::Json5 => sink.number(f64::NAN).map_err(abort),
                _ => Err(self.expected("a value", token.start)),
            },
            Kind::End => Err(self
                .lexer
                .error("unexpected end of the text", token.start)
                .into()),
            _ => Err(self.expected("a value", token.start)),
        }
    }

    fn expected<A>(&self, what: &str, at: usize) -> Failure<A> {
        let found = match self.text[at..].chars().next() {
            None => {
                return self
                    .lexer
                    .error(format!("expected {what}, found the end of the text"), at)
                    .into();
            }
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

    fn enter<A>(&self, depth: usize, at: usize) -> Result<(), Failure<A>> {
        if depth > MAX_DEPTH {
            return Err(Error::new(
                ErrorKind::Depth,
                format!("the text nests more than {MAX_DEPTH} levels deep"),
                self.text,
                at,
                self.dialect,
            )
            .into());
        }
        Ok(())
    }

    fn object<S: Sink>(&mut self, start: usize, sink: &mut S, depth: usize) -> Parsed<S::Abort> {
        self.enter(depth, start)?;
        sink.begin_object().map_err(Failure::Abort)?;
        let mut token = self.lexer.next()?;
        if token.kind == Kind::EndObject {
            return sink.end_object().map_err(Failure::Abort);
        }
        loop {
            let key = match token.kind {
                Kind::String(text) => text,
                Kind::Name { text, .. } if self.dialect == Dialect::Json5 => text,
                Kind::Name { .. } => {
                    return Err(self
                        .lexer
                        .error(
                            "a property name must be a double-quoted string",
                            token.start,
                        )
                        .into());
                }
                _ => return Err(self.expected("a property name", token.start)),
            };
            sink.key(key).map_err(Failure::Abort)?;
            let colon = self.lexer.next()?;
            if colon.kind != Kind::Colon {
                return Err(self.expected("':' after the property name", colon.start));
            }
            let value = self.lexer.next()?;
            self.value(value, sink, depth)?;
            let after = self.lexer.next()?;
            match after.kind {
                Kind::EndObject => break,
                Kind::Comma => {}
                _ => return Err(self.expected("',' or '}'", after.start)),
            }
            // One trailing comma may close the object.
            token = self.lexer.next()?;
            if token.kind == Kind::EndObject {
                break;
            }
        }
        sink.end_object().map_err(Failure::Abort)
    }

    fn array<S: Sink>(&mut self, start: usize, sink: &mut S, depth: usize) -> Parsed<S::Abort> {
        self.enter(depth, start)?;
        sink.begin_array().map_err(Failure::Abort)?;
        let mut token = self.lexer.next()?;
        if token.kind == Kind::EndArray {
            return sink.end_array().map_err(Failure::Abort);
        }
        loop {
            self.value(token, sink, depth)?;
            let after = self.lexer.next()?;
            match after.kind {
                Kind::EndArray => break,
                Kind::Comma => {}
                _ => return Err(self.expected("',' or ']'", after.start)),
            }
            // One trailing comma may close the array.
            token = self.lexer.next()?;
            if token.kind == Kind::EndArray {
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
