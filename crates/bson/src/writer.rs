//! Writing: a [`Writer`] that appends elements to an open document.
//!
//! Documents are written front to back with their length patched in when they
//! close, so a value is visited once and never measured first.

use crate::{Error, MAX_DEPTH, Result};

/// Appends BSON to a buffer. Open a document, write `key` then a value for each
/// element, and close it; nested documents open and close inside.
#[derive(Debug, Default)]
pub struct Writer {
    buf: Vec<u8>,
    /// Where each open document's length prefix is, innermost last.
    open: Vec<usize>,
}

impl Writer {
    /// A writer over an empty buffer.
    pub fn new() -> Self {
        Self::default()
    }

    /// How many documents are open.
    pub fn depth(&self) -> usize {
        self.open.len()
    }

    /// Opens a document, as the top level or as the value after a
    /// [`key`](Self::key) of type document, array or code-with-scope.
    pub fn start_document(&mut self) -> Result<()> {
        if self.open.len() >= MAX_DEPTH {
            return Err(Error::new(format!(
                "documents nest more than {MAX_DEPTH} deep"
            )));
        }
        self.open.push(self.buf.len());
        self.buf.extend_from_slice(&[0; 4]);
        Ok(())
    }

    /// Closes the innermost open document and writes its length.
    pub fn end_document(&mut self) -> Result<()> {
        let start = self
            .open
            .pop()
            .ok_or_else(|| Error::new("no document is open"))?;
        self.buf.push(0);
        self.patch_len(start)
    }

    fn patch_len(&mut self, start: usize) -> Result<()> {
        let len = i32::try_from(self.buf.len() - start)
            .map_err(|_| Error::new("a document is larger than 2 GiB"))?;
        self.buf[start..start + 4].copy_from_slice(&len.to_le_bytes());
        Ok(())
    }

    /// Starts an element: its type byte and name. A name may not contain NUL.
    pub fn key(&mut self, tag: u8, name: &str) -> Result<()> {
        if name.as_bytes().contains(&0) {
            return Err(Error::new(format!(
                "the field name {name:?} contains a NUL character"
            )));
        }
        self.buf.push(tag);
        self.buf.extend_from_slice(name.as_bytes());
        self.buf.push(0);
        Ok(())
    }

    /// Starts an array element's name, which is its index.
    pub fn index(&mut self, tag: u8, index: usize) {
        self.buf.push(tag);
        // `usize` formats without a NUL, so this cannot fail the way `key` can.
        self.buf.extend_from_slice(index.to_string().as_bytes());
        self.buf.push(0);
    }

    /// A double.
    pub fn double(&mut self, value: f64) {
        self.buf.extend_from_slice(&value.to_le_bytes());
    }

    /// An int32.
    pub fn int32(&mut self, value: i32) {
        self.buf.extend_from_slice(&value.to_le_bytes());
    }

    /// An int64, or a datetime's milliseconds.
    pub fn int64(&mut self, value: i64) {
        self.buf.extend_from_slice(&value.to_le_bytes());
    }

    /// A timestamp: the increment in the low 32 bits, the seconds in the high.
    pub fn timestamp(&mut self, value: u64) {
        self.buf.extend_from_slice(&value.to_le_bytes());
    }

    /// A boolean.
    pub fn boolean(&mut self, value: bool) {
        self.buf.push(u8::from(value));
    }

    /// A length-prefixed string, for string, code and symbol elements.
    pub fn string(&mut self, value: &str) -> Result<()> {
        let len = i32::try_from(value.len() + 1)
            .map_err(|_| Error::new("a string is larger than 2 GiB"))?;
        self.buf.extend_from_slice(&len.to_le_bytes());
        self.buf.extend_from_slice(value.as_bytes());
        self.buf.push(0);
        Ok(())
    }

    /// Raw bytes: an ObjectId's 12 or a Decimal128's 16.
    pub fn raw(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// A binary. Subtype 2 repeats its length inside the payload, as the format
    /// requires.
    pub fn binary(&mut self, subtype: u8, bytes: &[u8]) -> Result<()> {
        let extra = if subtype == 0x02 { 4 } else { 0 };
        let total = i32::try_from(bytes.len() + extra)
            .map_err(|_| Error::new("a binary is larger than 2 GiB"))?;
        self.buf.extend_from_slice(&total.to_le_bytes());
        self.buf.push(subtype);
        if subtype == 0x02 {
            // `total` fits, so the shorter inner length does too.
            self.buf.extend_from_slice(&(total - 4).to_le_bytes());
        }
        self.buf.extend_from_slice(bytes);
        Ok(())
    }

    /// A regular expression. Neither part may contain NUL; the options are
    /// written in alphabetical order, as the format requires.
    pub fn regex(&mut self, pattern: &str, options: &str) -> Result<()> {
        if pattern.as_bytes().contains(&0) || options.as_bytes().contains(&0) {
            return Err(Error::new(
                "a regular expression's pattern or options contain a NUL character",
            ));
        }
        let mut sorted: Vec<char> = options.chars().collect();
        sorted.sort_unstable();
        self.buf.extend_from_slice(pattern.as_bytes());
        self.buf.push(0);
        self.buf
            .extend_from_slice(sorted.into_iter().collect::<String>().as_bytes());
        self.buf.push(0);
        Ok(())
    }

    /// Opens a code-with-scope: its total length, then the code. Write the scope
    /// document next, then [`end_code_with_scope`](Self::end_code_with_scope).
    pub fn start_code_with_scope(&mut self, code: &str) -> Result<()> {
        self.open.push(self.buf.len());
        self.buf.extend_from_slice(&[0; 4]);
        self.string(code)
    }

    /// Closes a code-with-scope after its scope document.
    pub fn end_code_with_scope(&mut self) -> Result<()> {
        let start = self
            .open
            .pop()
            .ok_or_else(|| Error::new("no code-with-scope is open"))?;
        self.patch_len(start)
    }

    /// The bytes written. Every opened document must have been closed.
    pub fn finish(self) -> Result<Vec<u8>> {
        if !self.open.is_empty() {
            return Err(Error::new("a document was left open"));
        }
        Ok(self.buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Document, Element, kind};

    #[test]
    fn writes_what_the_reader_reads() {
        let mut w = Writer::new();
        w.start_document().unwrap();
        w.key(kind::INT32, "a").unwrap();
        w.int32(1);
        w.key(kind::ARRAY, "list").unwrap();
        w.start_document().unwrap();
        w.index(kind::STRING, 0);
        w.string("x").unwrap();
        w.end_document().unwrap();
        w.key(kind::BINARY, "old").unwrap();
        w.binary(0x02, &[0xff, 0xff]).unwrap();
        w.key(kind::REGEX, "r").unwrap();
        w.regex("ab", "mi").unwrap();
        w.end_document().unwrap();
        let bytes = w.finish().unwrap();

        crate::validate(&bytes).unwrap();
        let doc = Document::new(&bytes).unwrap();
        assert_eq!(doc.get("a").unwrap(), Some(Element::Int32(1)));
        assert_eq!(
            doc.get("old").unwrap(),
            Some(Element::Binary {
                subtype: 2,
                bytes: &[0xff, 0xff]
            })
        );
        assert_eq!(
            doc.get("r").unwrap(),
            Some(Element::Regex {
                pattern: "ab",
                options: "im"
            })
        );
    }

    #[test]
    fn a_nul_in_a_name_or_regex_is_refused() {
        let mut w = Writer::new();
        w.start_document().unwrap();
        assert!(w.key(kind::INT32, "a\0b").is_err());
        assert!(w.regex("a\0", "").is_err());
        assert!(w.regex("a", "i\0").is_err());
    }

    #[test]
    fn nesting_beyond_the_bound_is_refused() {
        let mut w = Writer::new();
        for _ in 0..MAX_DEPTH {
            w.start_document().unwrap();
        }
        assert!(w.start_document().is_err());
    }
}
