//! Reading: a framed [`Document`] and the [`Elements`] it yields.
//!
//! Framing is checked when a document is made (its length matches its bytes and
//! it ends in a zero) and each element is checked as it is reached, so a reader
//! that stops early has paid only for what it read. [`validate`] reaches every
//! element.

use crate::{Error, MAX_DEPTH, Result, kind};

/// One element's value, borrowing from the document it was read from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Element<'a> {
    /// 0x01.
    Double(f64),
    /// 0x02. Valid UTF-8; may contain NUL.
    String(&'a str),
    /// 0x03.
    Document(Document<'a>),
    /// 0x04. Its keys are not checked: a reader takes the values in order.
    Array(Document<'a>),
    /// 0x05. For subtype 2 the inner length has been checked and removed.
    Binary {
        /// The subtype byte.
        subtype: u8,
        /// The payload.
        bytes: &'a [u8],
    },
    /// 0x06, deprecated.
    Undefined,
    /// 0x07.
    ObjectId([u8; 12]),
    /// 0x08. Only 0 and 1 are read; anything else is an error.
    Boolean(bool),
    /// 0x09: milliseconds since the Unix epoch.
    DateTime(i64),
    /// 0x0A.
    Null,
    /// 0x0B.
    Regex {
        /// The pattern.
        pattern: &'a str,
        /// The options, as stored.
        options: &'a str,
    },
    /// 0x0C, deprecated.
    DbPointer {
        /// The namespace.
        namespace: &'a str,
        /// The id.
        id: [u8; 12],
    },
    /// 0x0D.
    Code(&'a str),
    /// 0x0E, deprecated.
    Symbol(&'a str),
    /// 0x0F, deprecated.
    CodeWithScope {
        /// The code.
        code: &'a str,
        /// The scope document.
        scope: Document<'a>,
    },
    /// 0x10.
    Int32(i32),
    /// 0x11: the increment in the low 32 bits, the seconds in the high 32.
    Timestamp(u64),
    /// 0x12.
    Int64(i64),
    /// 0x13: the 16 bytes as stored (little-endian).
    Decimal128([u8; 16]),
    /// 0xFF.
    MinKey,
    /// 0x7F.
    MaxKey,
}

/// A document whose framing has been checked: its declared length is its byte
/// length, and its last byte is the terminating zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Document<'a> {
    bytes: &'a [u8],
}

impl<'a> Document<'a> {
    /// Frames `bytes` as one whole document. Trailing bytes are an error.
    pub fn new(bytes: &'a [u8]) -> Result<Self> {
        let declared = read_len(bytes, 0)?;
        if declared != bytes.len() {
            return Err(Error::new(format!(
                "the document says it is {declared} bytes but {} were given",
                bytes.len()
            )));
        }
        Self::framed(bytes, 0)
    }

    /// Frames a document that starts at `bytes[0]` and may be followed by more
    /// bytes, returning it. `offset` is where `bytes` sits in the outermost
    /// document, for error messages.
    fn prefix(bytes: &'a [u8], offset: usize) -> Result<Self> {
        let declared = read_len(bytes, offset)?;
        if declared > bytes.len() {
            return Err(Error::at(
                offset,
                "a document runs past the end of its parent",
            ));
        }
        Self::framed(&bytes[..declared], offset)
    }

    fn framed(bytes: &'a [u8], offset: usize) -> Result<Self> {
        if bytes.len() < 5 {
            return Err(Error::at(offset, "a document is shorter than 5 bytes"));
        }
        if bytes[bytes.len() - 1] != 0 {
            return Err(Error::at(
                offset + bytes.len() - 1,
                "a document does not end in a zero byte",
            ));
        }
        Ok(Self { bytes })
    }

    /// The document's bytes, length prefix and terminator included.
    pub fn as_bytes(&self) -> &'a [u8] {
        self.bytes
    }

    /// The elements, in order. Each is checked as it is reached; the first
    /// malformed one ends the iteration with its error.
    pub fn iter(&self) -> Elements<'a> {
        Elements {
            bytes: self.bytes,
            pos: 4,
            done: false,
        }
    }

    /// The first element named `key`, or `None`. An error in an element read
    /// on the way is returned.
    pub fn get(&self, key: &str) -> Result<Option<Element<'a>>> {
        for element in self.iter() {
            let (name, value) = element?;
            if name == key {
                return Ok(Some(value));
            }
        }
        Ok(None)
    }
}

/// The elements of a [`Document`], as `(key, value)` pairs.
#[derive(Debug, Clone)]
pub struct Elements<'a> {
    bytes: &'a [u8],
    pos: usize,
    done: bool,
}

impl<'a> Iterator for Elements<'a> {
    type Item = Result<(&'a str, Element<'a>)>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        // The last byte is the document's terminator, checked when framed.
        if self.pos == self.bytes.len() - 1 {
            self.done = true;
            return None;
        }
        let item = self.read();
        if item.is_err() {
            self.done = true;
        }
        Some(item)
    }
}

impl<'a> Elements<'a> {
    fn read(&mut self) -> Result<(&'a str, Element<'a>)> {
        let bytes = self.bytes;
        // Everything before the terminator belongs to elements.
        let end = bytes.len() - 1;
        let start = self.pos;
        let tag = bytes[start];
        let (key, after_key) = cstring(bytes, start + 1, end)?;
        let mut cursor = Cursor {
            bytes,
            pos: after_key,
            end,
        };
        let value = match tag {
            kind::DOUBLE => Element::Double(f64::from_le_bytes(cursor.take::<8>()?)),
            kind::STRING => Element::String(cursor.string()?),
            kind::DOCUMENT => Element::Document(cursor.document()?),
            kind::ARRAY => Element::Array(cursor.document()?),
            kind::BINARY => cursor.binary()?,
            kind::UNDEFINED => Element::Undefined,
            kind::OBJECT_ID => Element::ObjectId(cursor.take::<12>()?),
            kind::BOOLEAN => match cursor.take::<1>()?[0] {
                0 => Element::Boolean(false),
                1 => Element::Boolean(true),
                other => {
                    return Err(Error::at(
                        cursor.pos - 1,
                        format_args!("a boolean is {other}, not 0 or 1"),
                    ));
                }
            },
            kind::DATETIME => Element::DateTime(i64::from_le_bytes(cursor.take::<8>()?)),
            kind::NULL => Element::Null,
            kind::REGEX => {
                let (pattern, next) = cstring(bytes, cursor.pos, end)?;
                let (options, next) = cstring(bytes, next, end)?;
                cursor.pos = next;
                Element::Regex { pattern, options }
            }
            kind::DB_POINTER => {
                let namespace = cursor.string()?;
                Element::DbPointer {
                    namespace,
                    id: cursor.take::<12>()?,
                }
            }
            kind::CODE => Element::Code(cursor.string()?),
            kind::SYMBOL => Element::Symbol(cursor.string()?),
            kind::CODE_WITH_SCOPE => cursor.code_with_scope()?,
            kind::INT32 => Element::Int32(i32::from_le_bytes(cursor.take::<4>()?)),
            kind::TIMESTAMP => Element::Timestamp(u64::from_le_bytes(cursor.take::<8>()?)),
            kind::INT64 => Element::Int64(i64::from_le_bytes(cursor.take::<8>()?)),
            kind::DECIMAL128 => Element::Decimal128(cursor.take::<16>()?),
            kind::MIN_KEY => Element::MinKey,
            kind::MAX_KEY => Element::MaxKey,
            other => {
                return Err(Error::at(
                    start,
                    format_args!("unknown element type 0x{other:02x}"),
                ));
            }
        };
        self.pos = cursor.pos;
        Ok((key, value))
    }
}

/// Reads within one document's element region, `bytes[pos..end]`.
struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
    end: usize,
}

impl<'a> Cursor<'a> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N]> {
        let slice = self.slice(N)?;
        let mut out = [0u8; N];
        out.copy_from_slice(slice);
        Ok(out)
    }

    fn slice(&mut self, len: usize) -> Result<&'a [u8]> {
        let stop = self
            .pos
            .checked_add(len)
            .filter(|&stop| stop <= self.end)
            .ok_or_else(|| Error::at(self.pos, "an element runs past the end of its document"))?;
        let out = &self.bytes[self.pos..stop];
        self.pos = stop;
        Ok(out)
    }

    fn i32(&mut self) -> Result<i32> {
        Ok(i32::from_le_bytes(self.take::<4>()?))
    }

    /// A length-prefixed string: a positive length counting the NUL, then that
    /// many bytes ending in NUL.
    fn string(&mut self) -> Result<&'a str> {
        let at = self.pos;
        let len = self.i32()?;
        let len = usize::try_from(len)
            .ok()
            .filter(|&len| len >= 1)
            .ok_or_else(|| Error::at(at, format_args!("a string length of {len}")))?;
        let raw = self.slice(len)?;
        let (text, nul) = raw.split_at(len - 1);
        if nul != [0] {
            return Err(Error::at(at, "a string does not end in a zero byte"));
        }
        std::str::from_utf8(text).map_err(|_| Error::at(at, "a string is not valid UTF-8"))
    }

    fn document(&mut self) -> Result<Document<'a>> {
        let doc = Document::prefix(&self.bytes[self.pos..self.end], self.pos)?;
        self.pos += doc.bytes.len();
        Ok(doc)
    }

    fn binary(&mut self) -> Result<Element<'a>> {
        let at = self.pos;
        let len = self.i32()?;
        let len = usize::try_from(len)
            .map_err(|_| Error::at(at, format_args!("a binary length of {len}")))?;
        let subtype = self.take::<1>()?[0];
        let bytes = self.slice(len)?;
        if subtype == 0x02 {
            // The old binary subtype repeats the length inside the payload.
            let inner = bytes
                .get(..4)
                .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
                .ok_or_else(|| {
                    Error::at(at, "a subtype 2 binary is shorter than its own length")
                })?;
            if usize::try_from(inner).ok() != Some(len - 4) {
                return Err(Error::at(
                    at,
                    format_args!("a subtype 2 binary says {inner} bytes inside {len}"),
                ));
            }
            return Ok(Element::Binary {
                subtype,
                bytes: &bytes[4..],
            });
        }
        Ok(Element::Binary { subtype, bytes })
    }

    /// The total length covers itself, the string and the scope exactly.
    fn code_with_scope(&mut self) -> Result<Element<'a>> {
        let at = self.pos;
        let total = self.i32()?;
        let total = usize::try_from(total)
            .ok()
            .filter(|&total| total >= 14)
            .ok_or_else(|| Error::at(at, format_args!("a code-with-scope length of {total}")))?;
        let stop = at
            .checked_add(total)
            .filter(|&stop| stop <= self.end)
            .ok_or_else(|| Error::at(at, "a code-with-scope runs past the end of its document"))?;
        let mut inner = Cursor {
            bytes: self.bytes,
            pos: self.pos,
            end: stop,
        };
        let code = inner.string()?;
        let scope = inner.document()?;
        if inner.pos != stop {
            return Err(Error::at(
                at,
                "a code-with-scope's length does not match its contents",
            ));
        }
        self.pos = stop;
        Ok(Element::CodeWithScope { code, scope })
    }
}

/// The document length at `bytes[0..4]`, which must be non-negative.
fn read_len(bytes: &[u8], offset: usize) -> Result<usize> {
    let head = bytes
        .get(..4)
        .ok_or_else(|| Error::at(offset, "a document is shorter than its length prefix"))?;
    let len = i32::from_le_bytes([head[0], head[1], head[2], head[3]]);
    usize::try_from(len).map_err(|_| Error::at(offset, format_args!("a document length of {len}")))
}

/// A NUL-terminated UTF-8 string starting at `start`, ending before `end`.
/// Returns it and the offset after its NUL.
fn cstring(bytes: &[u8], start: usize, end: usize) -> Result<(&str, usize)> {
    let region = bytes
        .get(start..end)
        .ok_or_else(|| Error::at(start, "a name runs past the end of its document"))?;
    let len = region
        .iter()
        .position(|&b| b == 0)
        .ok_or_else(|| Error::at(start, "a name is not terminated"))?;
    let text = std::str::from_utf8(&region[..len])
        .map_err(|_| Error::at(start, "a name is not valid UTF-8"))?;
    Ok((text, start + len + 1))
}

/// Checks a whole document: framing, every element, and every nested document
/// to [`MAX_DEPTH`].
pub fn validate(bytes: &[u8]) -> Result<()> {
    walk(Document::new(bytes)?, 1)
}

fn walk(doc: Document<'_>, depth: usize) -> Result<()> {
    if depth > MAX_DEPTH {
        return Err(Error::new(format!(
            "documents nest more than {MAX_DEPTH} deep"
        )));
    }
    for element in doc.iter() {
        match element?.1 {
            Element::Document(inner) | Element::Array(inner) => walk(inner, depth + 1)?,
            Element::CodeWithScope { scope, .. } => walk(scope, depth + 1)?,
            _ => {}
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(text: &str) -> Vec<u8> {
        text.as_bytes()
            .chunks(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }

    #[test]
    fn reads_elements_in_order_and_keeps_duplicates() {
        // { foo: 1, foo: 2 }: the reader returns both, in order.
        let bytes = hex("1700000010666f6f000100000010666f6f000200000000");
        let doc = Document::new(&bytes).unwrap();
        let got: Vec<_> = doc.iter().map(Result::unwrap).collect();
        assert_eq!(
            got,
            [("foo", Element::Int32(1)), ("foo", Element::Int32(2))]
        );
        assert_eq!(doc.get("foo").unwrap(), Some(Element::Int32(1)));
    }

    #[test]
    fn framing_is_checked_before_any_element() {
        assert!(Document::new(&hex("0500000000")).is_ok());
        assert!(Document::new(&hex("0500000001")).is_err(), "no terminator");
        assert!(
            Document::new(&hex("0600000000")).is_err(),
            "longer than given"
        );
        assert!(
            Document::new(&hex("050000000000")).is_err(),
            "trailing bytes"
        );
        assert!(
            Document::new(&hex("FFFFFFFF00")).is_err(),
            "negative length"
        );
    }

    #[test]
    fn a_bad_element_ends_the_iteration_with_its_error() {
        // A boolean of 2.
        let bytes = hex("090000000862000200");
        let mut elements = Document::new(&bytes).unwrap().iter();
        assert!(elements.next().unwrap().is_err());
        assert!(elements.next().is_none());
    }

    #[test]
    fn nesting_beyond_the_bound_is_refused() {
        // { a: { a: { … } } }, MAX_DEPTH + 1 levels.
        let mut doc = hex("0500000000");
        for _ in 0..MAX_DEPTH {
            let mut outer = Vec::new();
            let len = 4 + 1 + 2 + doc.len() + 1;
            outer.extend_from_slice(&i32::try_from(len).unwrap().to_le_bytes());
            outer.extend_from_slice(&[kind::DOCUMENT, b'a', 0]);
            outer.extend_from_slice(&doc);
            outer.push(0);
            doc = outer;
        }
        assert!(validate(&doc).is_err());
    }
}
