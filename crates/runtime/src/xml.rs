//! The XML reader and writer behind `runtime:serialization`'s `XML` namespace,
//! over the runtime's [`Value`] tree.
//!
//! quick-xml is the tokenizer; everything that decides what a document *means*
//! is here, in one place that `parse`, `validate`, the streaming decoder and the
//! fuzz target all share. That is the point of the module: the three used to
//! walk events separately, and each dropped something the others kept — a
//! `&amp;` (quick-xml reports references as their own event), a CDATA section,
//! an error inside a streamed record.
//!
//! The value shape (see DECISIONS.md D175): an element is a key; an attribute
//! is `@name`; an element's own text is `$text`; a name that repeats becomes an
//! array; an element with no attributes, children or text is `""`, however it
//! is spelled (`<a/>` and `<a></a>` are the same element). The document is an
//! object with the root element as its one key.
//!
//! What the reader requires is XML 1.0 well-formedness short of a DTD: one root
//! element, nothing but whitespace outside it, names that are XML names,
//! attributes unique and free of `<`, every reference resolvable. A DTD is
//! skipped, not processed, so a reference to an entity it declares is an error
//! rather than silently becoming nothing.
//!
//! The writer is the reader's inverse over that shape and refuses what it
//! cannot spell: a name that is not an XML name, a character XML 1.0 cannot
//! carry, bytes, a non-finite number, an array with no element name to repeat.

use es_runtime_engine::Value;
use quick_xml::Reader;
use quick_xml::XmlVersion;
use quick_xml::errors::{IllFormedError, SyntaxError};
use quick_xml::events::{BytesStart, Event};
use std::fmt;

/// Maximum element nesting. 256 matches libxml2's default and is far deeper
/// than any realistic document; past it a hostile document is refused rather
/// than allowed to grow the element stack without bound.
pub(crate) const MAX_DEPTH: usize = 256;

/// Cap on a stream's retained (unconsumed) bytes. The decoder holds a record's
/// bytes until it closes; one that never closes is refused here instead of
/// growing without bound.
pub(crate) const MAX_STREAM_BUFFER: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ErrorKind {
    /// The input is not well-formed XML.
    Syntax,
    /// A value given to `build` cannot be written as XML.
    Type,
    /// A stream's unclosed record passed [`MAX_STREAM_BUFFER`].
    Limit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Error {
    pub kind: ErrorKind,
    pub message: String,
}

impl Error {
    fn syntax(message: impl Into<String>) -> Error {
        Error {
            kind: ErrorKind::Syntax,
            message: message.into(),
        }
    }

    fn type_(message: impl Into<String>) -> Error {
        Error {
            kind: ErrorKind::Type,
            message: message.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl From<quick_xml::Error> for Error {
    fn from(e: quick_xml::Error) -> Error {
        Error::syntax(e.to_string())
    }
}

impl From<quick_xml::encoding::EncodingError> for Error {
    fn from(e: quick_xml::encoding::EncodingError) -> Error {
        Error::syntax(e.to_string())
    }
}

impl From<quick_xml::events::attributes::AttrError> for Error {
    fn from(e: quick_xml::events::attributes::AttrError) -> Error {
        Error::syntax(e.to_string())
    }
}

// ---- names and characters --------------------------------------------------

/// XML 1.0 `NameStartChar`.
fn is_name_start(c: char) -> bool {
    matches!(c,
        ':' | 'A'..='Z' | '_' | 'a'..='z'
        | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}' | '\u{F8}'..='\u{2FF}'
        | '\u{370}'..='\u{37D}' | '\u{37F}'..='\u{1FFF}' | '\u{200C}'..='\u{200D}'
        | '\u{2070}'..='\u{218F}' | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}'
        | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}' | '\u{10000}'..='\u{EFFFF}')
}

/// XML 1.0 `NameChar`.
fn is_name_char(c: char) -> bool {
    is_name_start(c)
        || matches!(c,
            '-' | '.' | '0'..='9' | '\u{B7}' | '\u{300}'..='\u{36F}' | '\u{203F}'..='\u{2040}')
}

/// Whether `s` is an XML 1.0 `Name`.
fn is_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars.next().is_some_and(is_name_start) && chars.all(is_name_char)
}

/// XML 1.0 `Char`: what a document may contain at all, even as a reference.
fn is_xml_char(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\r' | '\u{20}'..='\u{D7FF}' | '\u{E000}'..='\u{FFFD}' | '\u{10000}'..)
}

/// XML's whitespace (`S`): space, tab, CR, LF — not Unicode's wider set.
fn is_xml_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\r' | '\n')
}

fn checked_name(raw: &[u8]) -> Result<String, Error> {
    let name = std::str::from_utf8(raw).map_err(|_| Error::syntax("a name is not valid UTF-8"))?;
    if !is_name(name) {
        return Err(Error::syntax(format!("{name:?} is not an XML name")));
    }
    Ok(name.to_string())
}

// ---- reading ---------------------------------------------------------------

/// An element's text as it accumulates.
///
/// Indentation is not content, so leading and trailing whitespace is trimmed —
/// but only whitespace that was written as plain text. A CDATA section or a
/// reference (`&#32;`) is the author spelling a character out on purpose, so
/// trimming stops at the first and last of those.
#[derive(Default)]
struct Text {
    buf: String,
    /// Byte range `[first, last)` spanned by CDATA and references, if any.
    literal: Option<(usize, usize)>,
}

impl Text {
    fn push_plain(&mut self, s: &str) {
        self.buf.push_str(s);
    }

    fn push_literal(&mut self, s: &str) {
        let start = self.buf.len();
        self.buf.push_str(s);
        let end = self.buf.len();
        self.literal = Some(match self.literal {
            Some((first, _)) => (first, end),
            None => (start, end),
        });
    }

    fn finish(self) -> String {
        let Some((first, last)) = self.literal else {
            return self.buf.trim_matches(is_xml_space).to_string();
        };
        let lead = self.buf[..first]
            .find(|c: char| !is_xml_space(c))
            .unwrap_or(first);
        let tail = self.buf[last..]
            .rfind(|c: char| !is_xml_space(c))
            .map_or(last, |i| {
                let c = self.buf[last + i..].chars().next().expect("found a char");
                last + i + c.len_utf8()
            });
        self.buf[lead..tail].to_string()
    }
}

struct Frame {
    name: String,
    /// Attributes then child elements, in document order within each.
    attrs: Vec<(String, Value)>,
    children: Vec<(String, Value)>,
    text: Text,
}

impl Frame {
    fn open(e: &BytesStart<'_>) -> Result<Frame, Error> {
        Ok(Frame {
            name: checked_name(e.name().as_ref())?,
            attrs: read_attrs(e)?,
            children: Vec::new(),
            text: Text::default(),
        })
    }

    /// The element's value: `""` when it has nothing at all, else an object of
    /// its attributes, children and (when non-empty) `$text`.
    fn close(self) -> (String, Value) {
        let text = self.text.finish();
        if self.attrs.is_empty() && self.children.is_empty() && text.is_empty() {
            return (self.name, Value::String(String::new()));
        }
        let mut entries = self.attrs;
        entries.extend(self.children);
        if !text.is_empty() {
            entries.push(("$text".to_string(), Value::String(text)));
        }
        (self.name, Value::Object(entries))
    }

    /// Adds a child element; a name seen before turns into an array.
    fn add_child(&mut self, name: String, value: Value) {
        match self.children.iter_mut().find(|(k, _)| *k == name) {
            Some((_, Value::Array(items))) => items.push(value),
            Some((_, existing)) => {
                let first = std::mem::replace(existing, Value::Null);
                *existing = Value::Array(vec![first, value]);
            }
            None => self.children.push((name, value)),
        }
    }
}

fn read_attrs(e: &BytesStart<'_>) -> Result<Vec<(String, Value)>, Error> {
    let mut attrs = Vec::new();
    // Checks are on by default, so a repeated attribute is an `Err` here — the
    // old `.flatten()` dropped it and kept whichever came first.
    for attr in e.attributes() {
        let attr = attr?;
        let name = checked_name(attr.key.as_ref())?;
        if attr.value.contains(&b'<') {
            return Err(Error::syntax(format!(
                "`<` in the value of attribute {name:?}"
            )));
        }
        // Resolves the predefined entities and character references and
        // applies attribute-value normalization; an unknown entity is an error.
        let value = attr.normalized_value(XmlVersion::Implicit1_0)?;
        attrs.push((format!("@{name}"), Value::String(value.into_owned())));
    }
    Ok(attrs)
}

/// The one character a reference stands for: a character reference, or one of
/// the five predefined entities. Anything else would need a DTD.
fn resolve_reference(e: &quick_xml::events::BytesRef<'_>) -> Result<char, Error> {
    if let Some(c) = e.resolve_char_ref()? {
        if !is_xml_char(c) {
            return Err(Error::syntax(format!(
                "character reference to U+{:04X}, which XML cannot contain",
                c as u32
            )));
        }
        return Ok(c);
    }
    match e.decode()?.as_ref() {
        "amp" => Ok('&'),
        "lt" => Ok('<'),
        "gt" => Ok('>'),
        "quot" => Ok('"'),
        "apos" => Ok('\''),
        other => Err(Error::syntax(format!(
            "unknown entity &{other}; (a DTD is not processed)"
        ))),
    }
}

/// Parses a document to the value tree described in the module docs.
pub(crate) fn parse(input: &str) -> Result<Value, Error> {
    parse_at(input, 0)
}

/// Checks that `input` is a document `parse` would accept.
pub(crate) fn validate(input: &str) -> Result<(), Error> {
    parse(input).map(|_| ())
}

/// [`parse`] for a document that sits `base_depth` levels inside another — a
/// streamed record is a child of the root, so it has one level less to spend.
fn parse_at(input: &str, base_depth: usize) -> Result<Value, Error> {
    let mut reader = Reader::from_str(input);
    reader.config_mut().check_comments = true;

    let mut stack: Vec<Frame> = Vec::new();
    let mut root: Option<(String, Value)> = None;

    loop {
        let event = reader.read_event()?;
        match event {
            Event::Start(ref e) | Event::Empty(ref e) => {
                if root.is_some() {
                    return Err(Error::syntax("more than one root element"));
                }
                if base_depth + stack.len() >= MAX_DEPTH {
                    return Err(Error::syntax(format!(
                        "nesting deeper than {MAX_DEPTH} levels"
                    )));
                }
                let frame = Frame::open(e)?;
                if matches!(event, Event::Start(_)) {
                    stack.push(frame);
                } else {
                    let (name, value) = frame.close();
                    match stack.last_mut() {
                        Some(parent) => parent.add_child(name, value),
                        None => root = Some((name, value)),
                    }
                }
            }
            // The reader has already matched the end tag's name to the open one.
            Event::End(_) => {
                let frame = stack.pop().expect("the reader matched an open element");
                let (name, value) = frame.close();
                match stack.last_mut() {
                    Some(parent) => parent.add_child(name, value),
                    None => root = Some((name, value)),
                }
            }
            Event::Text(e) => {
                let text = e.xml10_content()?;
                match stack.last_mut() {
                    Some(frame) => frame.text.push_plain(&text),
                    None if text.chars().all(is_xml_space) => {}
                    None => return Err(Error::syntax("text outside the root element")),
                }
            }
            Event::CData(e) => match stack.last_mut() {
                Some(frame) => frame.text.push_literal(&e.xml10_content()?),
                None => return Err(Error::syntax("CDATA outside the root element")),
            },
            Event::GeneralRef(e) => match stack.last_mut() {
                Some(frame) => {
                    let mut buf = [0u8; 4];
                    frame
                        .text
                        .push_literal(resolve_reference(&e)?.encode_utf8(&mut buf));
                }
                None => return Err(Error::syntax("a reference outside the root element")),
            },
            Event::Eof => {
                if let Some(open) = stack.last() {
                    return Err(Error::syntax(format!(
                        "unexpected end of input inside <{}>",
                        open.name
                    )));
                }
                return match root {
                    Some((name, value)) => Ok(Value::Object(vec![(name, value)])),
                    None => Err(Error::syntax("no root element")),
                };
            }
            // The declaration, comments, processing instructions and a DOCTYPE
            // carry no data in this shape.
            Event::Decl(_) | Event::Comment(_) | Event::PI(_) | Event::DocType(_) => {}
        }
    }
}

// ---- streaming ---------------------------------------------------------------

/// `XML.DecoderStream`'s state: emits one value per direct child of the root,
/// each parsed by [`parse_at`] — the same reader as `parse`, so a record is
/// held to the same rules and an error in one is an error in the stream.
///
/// The scan only finds where records begin and end. It keeps the bytes of the
/// record in progress (from `buffer`'s start) and the structure around it, and
/// re-reads from the record's start when more bytes arrive.
pub(crate) struct StreamDecoder {
    buffer: Vec<u8>,
    /// 0 before the root opens and after it closes; 1 inside the root between
    /// records. Nesting inside a record is the record parser's business.
    depth: usize,
    root: Option<String>,
    root_closed: bool,
}

impl StreamDecoder {
    pub(crate) fn new() -> StreamDecoder {
        StreamDecoder {
            buffer: Vec::new(),
            depth: 0,
            root: None,
            root_closed: false,
        }
    }

    pub(crate) fn push(&mut self, chunk: &str) -> Result<Vec<Value>, Error> {
        self.buffer.extend_from_slice(chunk.as_bytes());
        let out = self.scan()?;
        if self.buffer.len() > MAX_STREAM_BUFFER {
            return Err(Error {
                kind: ErrorKind::Limit,
                message: format!("unclosed record exceeds {MAX_STREAM_BUFFER} bytes"),
            });
        }
        Ok(out)
    }

    /// Checks the end of the document: the root must have been seen and closed,
    /// and nothing but whitespace may be left.
    pub(crate) fn finish(&mut self) -> Result<(), Error> {
        self.scan()?;
        let rest = String::from_utf8_lossy(&self.buffer);
        if !rest.chars().all(is_xml_space) || (self.root.is_some() && !self.root_closed) {
            let inside = self.root.as_deref().unwrap_or("the document");
            return Err(Error::syntax(format!(
                "unexpected end of input inside <{inside}>"
            )));
        }
        if self.root.is_none() {
            return Err(Error::syntax("no root element"));
        }
        Ok(())
    }

    fn scan(&mut self) -> Result<Vec<Value>, Error> {
        let mut out = Vec::new();
        // Records are checked by `parse_at`; the scan only needs structure, and
        // it starts mid-document, where the root's end tag is unmatched.
        let mut reader = Reader::from_reader(&self.buffer[..]);
        reader.config_mut().check_end_names = false;
        reader.config_mut().allow_unmatched_ends = true;

        let mut consumed = 0;
        let mut depth = self.depth;
        let mut record_start = None;

        loop {
            let before = reader.buffer_position() as usize;
            let event = match reader.read_event() {
                Ok(event) => event,
                Err(e)
                    if is_incomplete(&e, self.buffer.len() - reader.error_position() as usize) =>
                {
                    break;
                }
                Err(e) => return Err(e.into()),
            };
            let after = reader.buffer_position() as usize;
            match event {
                Event::Start(ref e) | Event::Empty(ref e) => {
                    let empty = matches!(event, Event::Empty(_));
                    match depth {
                        0 => {
                            if self.root_closed {
                                return Err(Error::syntax("more than one root element"));
                            }
                            self.root = Some(checked_name(e.name().as_ref())?);
                            read_attrs(e)?;
                            if empty {
                                self.root_closed = true;
                            } else {
                                depth = 1;
                            }
                            consumed = after;
                        }
                        1 if empty => {
                            out.push(self.record(before, after)?);
                            consumed = after;
                        }
                        1 => {
                            record_start = Some(before);
                            depth = 2;
                        }
                        _ => depth += 1,
                    }
                }
                Event::End(ref e) => match depth {
                    1 => {
                        let name = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                        if self.root.as_deref() != Some(name.as_str()) {
                            return Err(Error::syntax(format!(
                                "expected </{}>, found </{name}>",
                                self.root.as_deref().unwrap_or_default()
                            )));
                        }
                        depth = 0;
                        self.root_closed = true;
                        consumed = after;
                    }
                    2 => {
                        let start = record_start.take().expect("a record was open");
                        out.push(self.record(start, after)?);
                        depth = 1;
                        consumed = after;
                    }
                    0 => return Err(Error::syntax("an end tag outside the root element")),
                    _ => depth -= 1,
                },
                Event::Text(ref e) if depth == 0 => {
                    if !e.xml10_content()?.chars().all(is_xml_space) {
                        return Err(Error::syntax("text outside the root element"));
                    }
                    consumed = after;
                }
                Event::CData(_) | Event::GeneralRef(_) if depth == 0 => {
                    return Err(Error::syntax("text outside the root element"));
                }
                Event::Eof => break,
                // Text directly inside the root, between records, is not part
                // of any record; inside a record the record parser reads it.
                _ => {
                    if depth <= 1 {
                        consumed = after;
                    }
                }
            }
        }

        self.buffer.drain(..consumed);
        self.depth = if depth >= 2 { 1 } else { depth };
        Ok(out)
    }

    fn record(&self, start: usize, end: usize) -> Result<Value, Error> {
        // The bytes came from `&str` chunks and the slice ends at a tag
        // boundary, so they are UTF-8; the check documents the invariant.
        let text = std::str::from_utf8(&self.buffer[start..end])
            .map_err(|_| Error::syntax("a record is not valid UTF-8"))?;
        parse_at(text, 1)
    }
}

/// Whether a scan error only means the buffer stops mid-construct — more bytes
/// may complete it — rather than that the document is malformed. `remaining` is
/// how many bytes followed the error position.
fn is_incomplete(e: &quick_xml::Error, remaining: usize) -> bool {
    match e {
        quick_xml::Error::Syntax(
            SyntaxError::UnclosedPI
            | SyntaxError::UnclosedXmlDecl
            | SyntaxError::UnclosedComment
            | SyntaxError::UnclosedDoctype
            | SyntaxError::UnclosedCData
            | SyntaxError::UnclosedTag
            | SyntaxError::UnclosedSingleQuotedAttributeValue
            | SyntaxError::UnclosedDoubleQuotedAttributeValue,
        ) => true,
        // `<!` and too few bytes to tell a comment, CDATA or DOCTYPE apart.
        quick_xml::Error::Syntax(SyntaxError::InvalidBangMarkup) => remaining < 9,
        quick_xml::Error::IllFormed(IllFormedError::UnclosedReference) => true,
        _ => false,
    }
}

// ---- writing ---------------------------------------------------------------

/// Writes a value as an XML document: an object with one element key is that
/// element; anything else is wrapped in `<root>`.
pub(crate) fn emit(value: &Value) -> Result<String, Error> {
    let (name, inner) = match value {
        Value::Object(entries)
            if entries.len() == 1 && !entries[0].0.starts_with('@') && entries[0].0 != "$text" =>
        {
            (entries[0].0.as_str(), &entries[0].1)
        }
        _ => ("root", value),
    };
    if matches!(inner, Value::Array(_)) {
        return Err(Error::type_(format!(
            "an array under {name:?} would write several root elements; XML has one"
        )));
    }
    let mut out = String::new();
    write_element(&mut out, name, inner, 0)?;
    Ok(out)
}

fn write_element(out: &mut String, name: &str, value: &Value, depth: usize) -> Result<(), Error> {
    if depth >= MAX_DEPTH {
        return Err(Error::type_(format!(
            "value nests deeper than {MAX_DEPTH} levels"
        )));
    }
    if !is_name(name) {
        return Err(Error::type_(format!("{name:?} is not an XML name")));
    }
    out.push('<');
    out.push_str(name);
    match value {
        Value::Null | Value::Undefined => out.push_str("/>"),
        Value::Object(entries) => {
            let mut has_content = false;
            for (key, v) in entries {
                if let Some(attr) = key.strip_prefix('@') {
                    if !is_name(attr) {
                        return Err(Error::type_(format!("{attr:?} is not an XML name")));
                    }
                    // An absent attribute is not written, as `parse` never
                    // produces one.
                    if matches!(v, Value::Null | Value::Undefined) {
                        continue;
                    }
                    out.push(' ');
                    out.push_str(attr);
                    out.push_str("=\"");
                    escape(out, &scalar_text(v, key)?, true)?;
                    out.push('"');
                } else {
                    has_content = true;
                }
            }
            if !has_content {
                out.push_str("/>");
                return Ok(());
            }
            out.push('>');
            for (key, v) in entries {
                if key.starts_with('@') {
                    continue;
                }
                if key == "$text" {
                    write_text(out, &scalar_text(v, key)?)?;
                    continue;
                }
                match v {
                    Value::Array(items) => {
                        for item in items {
                            if matches!(item, Value::Array(_)) {
                                return Err(Error::type_(format!(
                                    "an array inside the array under {key:?} has no element name"
                                )));
                            }
                            write_element(out, key, item, depth + 1)?;
                        }
                    }
                    _ => write_element(out, key, v, depth + 1)?,
                }
            }
            out.push_str("</");
            out.push_str(name);
            out.push('>');
        }
        Value::Array(_) => {
            return Err(Error::type_(format!(
                "an array under {name:?} has no element name to repeat"
            )));
        }
        scalar => {
            let text = scalar_text(scalar, name)?;
            if text.is_empty() {
                out.push_str("/>");
            } else {
                out.push('>');
                write_text(out, &text)?;
                out.push_str("</");
                out.push_str(name);
                out.push('>');
            }
        }
    }
    Ok(())
}

/// The text a scalar writes as: `String(v)` for a number or boolean, a
/// `BigInt`'s exact digits.
fn scalar_text(v: &Value, key: &str) -> Result<String, Error> {
    match v {
        Value::String(s) => Ok(s.clone()),
        Value::Bool(b) => Ok(b.to_string()),
        Value::Number(n) if n.is_finite() => Ok(crate::js_number::to_js_string(*n)),
        Value::Number(_) => Err(Error::type_(format!(
            "{key:?} is NaN or Infinity, which XML has no form for"
        ))),
        Value::BigInt(i) => Ok(i.to_string()),
        Value::Null | Value::Undefined => Ok(String::new()),
        Value::Bytes(_) => Err(Error::type_(format!(
            "{key:?} holds bytes, which XML text cannot carry"
        ))),
        _ => Err(Error::type_(format!(
            "{key:?} cannot be written as XML text"
        ))),
    }
}

/// Writes element text. The reader trims whitespace written as plain text at
/// either edge (it is indentation), so whitespace there is written as character
/// references, which the reader keeps.
fn write_text(out: &mut String, s: &str) -> Result<(), Error> {
    let start = s.find(|c: char| !is_xml_space(c)).unwrap_or(s.len());
    let end = s.rfind(|c: char| !is_xml_space(c)).map_or(start, |i| {
        i + s[i..].chars().next().expect("found a char").len_utf8()
    });
    for c in s[..start].chars() {
        push_char_ref(out, c);
    }
    escape(out, &s[start..end], false)?;
    for c in s[end..].chars() {
        push_char_ref(out, c);
    }
    Ok(())
}

fn push_char_ref(out: &mut String, c: char) {
    out.push_str("&#");
    out.push_str(&(c as u32).to_string());
    out.push(';');
}

/// Escapes text (or an attribute value) so the reader gives it back unchanged.
///
/// `\r` is a reference because the reader normalizes line ends; in an attribute
/// `\t` and `\n` are too, because attribute-value normalization would turn them
/// into spaces. A character XML 1.0 cannot contain at all is refused.
fn escape(out: &mut String, s: &str, attribute: bool) -> Result<(), Error> {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            // `>` only matters in `]]>`, but escaping it always is simpler.
            '>' => out.push_str("&gt;"),
            '"' if attribute => out.push_str("&quot;"),
            '\r' => out.push_str("&#13;"),
            '\n' if attribute => out.push_str("&#10;"),
            '\t' if attribute => out.push_str("&#9;"),
            c if !is_xml_char(c) => {
                return Err(Error::type_(format!(
                    "U+{:04X} cannot appear in an XML document",
                    c as u32
                )));
            }
            c => out.push(c),
        }
    }
    Ok(())
}

/// Fuzz entry: parse arbitrary text and, when it parses, write it back and read
/// it again — the writer must produce what the reader accepts, and the same
/// value.
#[cfg(any(test, feature = "fuzzing"))]
pub(crate) fn fuzz_round_trip(input: &str) {
    if let Ok(value) = parse(input) {
        let text = emit(&value).expect("a parsed document must re-emit");
        let again = parse(&text).expect("the writer's own output must parse");
        debug_assert_eq!(again, value, "input {input:?}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> Value {
        Value::String(v.to_string())
    }

    fn obj(pairs: &[(&str, Value)]) -> Value {
        Value::Object(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        )
    }

    fn doc(root: &str, v: Value) -> Value {
        obj(&[(root, v)])
    }

    fn err(input: &str) -> String {
        parse(input).expect_err("must fail").message
    }

    #[test]
    fn the_documented_shape() {
        assert_eq!(
            parse("<r a=\"1\">text</r>").unwrap(),
            doc("r", obj(&[("@a", s("1")), ("$text", s("text"))]))
        );
        assert_eq!(
            parse("<r><i>1</i><i>2</i></r>").unwrap(),
            doc(
                "r",
                obj(&[(
                    "i",
                    Value::Array(vec![obj(&[("$text", s("1"))]), obj(&[("$text", s("2"))])])
                )])
            )
        );
        assert_eq!(
            parse("<r><e/></r>").unwrap(),
            doc("r", obj(&[("e", s(""))]))
        );
    }

    #[test]
    fn an_empty_element_is_empty_however_it_is_spelled() {
        let expected = doc("r", obj(&[("a", s("")), ("b", s("")), ("c", s(""))]));
        assert_eq!(parse("<r><a></a><b/><c>  \n </c></r>").unwrap(), expected);
    }

    #[test]
    fn references_in_text_and_attributes_resolve() {
        // quick-xml reports `&amp;` as its own event; ignoring it dropped the
        // character, so `t&lt;` parsed to `t`.
        assert_eq!(
            parse("<a x=\"&amp;&lt;&#65;&#x42;\">t&lt;&amp;&gt;&quot;&apos;&#169;</a>").unwrap(),
            doc("a", obj(&[("@x", s("&<AB")), ("$text", s("t<&>\"'©"))]))
        );
        assert_eq!(
            parse("<a>a &amp; b</a>").unwrap(),
            doc("a", obj(&[("$text", s("a & b"))]))
        );
    }

    #[test]
    fn cdata_is_text_and_keeps_its_whitespace() {
        assert_eq!(
            parse("<a><![CDATA[x<y]]></a>").unwrap(),
            doc("a", obj(&[("$text", s("x<y"))]))
        );
        assert_eq!(
            parse("<a>\n  pre<![CDATA[ & ]]>post  \n</a>").unwrap(),
            doc("a", obj(&[("$text", s("pre & post"))]))
        );
        // A literal at the edge keeps the whitespace it spells out.
        assert_eq!(
            parse("<a>  <![CDATA[  x  ]]>  </a>").unwrap(),
            doc("a", obj(&[("$text", s("  x  "))]))
        );
        assert_eq!(
            parse("<a>&#32;x</a>").unwrap(),
            doc("a", obj(&[("$text", s(" x"))]))
        );
    }

    #[test]
    fn plain_text_is_trimmed_at_both_edges() {
        assert_eq!(
            parse("<a>\n  x  y  \n</a>").unwrap(),
            doc("a", obj(&[("$text", s("x  y"))]))
        );
    }

    #[test]
    fn line_ends_are_normalized() {
        assert_eq!(
            parse("<a>x\r\ny\rz</a>").unwrap(),
            doc("a", obj(&[("$text", s("x\ny\nz"))]))
        );
    }

    #[test]
    fn malformed_documents_are_refused() {
        assert!(err("<a/><b/>").contains("more than one root"));
        assert!(err("<a/>junk").contains("outside the root"));
        assert!(err("junk<a/>").contains("outside the root"));
        assert!(err("<a/><![CDATA[x]]>").contains("outside the root"));
        assert!(err("<a/>&amp;").contains("outside the root"));
        assert!(err("<a>&foo;</a>").contains("unknown entity"));
        assert!(err("<a x=\"&foo;\"/>").contains("foo"));
        assert!(parse("<a>&#0;</a>").is_err());
        assert!(err("<a>&#xFFFE;</a>").contains("U+FFFE"));
        assert!(!err("<a x=\"1\" x=\"2\"/>").is_empty());
        assert!(err("<a x=\"<\"/>").contains('<'));
        assert!(err("<1a/>").contains("not an XML name"));
        assert!(err("<a><b></a>").contains('b') || err("<a><b></a>").contains('a'));
        assert!(err("<r>").contains("<r>"));
        assert!(err("").contains("no root"));
        assert!(err("<!-- only -->").contains("no root"));
    }

    #[test]
    fn prolog_and_trailing_misc_are_allowed() {
        assert_eq!(
            parse("\u{feff}<?xml version=\"1.0\"?>\n<!DOCTYPE a>\n<!-- c --><a/>\n<!-- t -->\n")
                .unwrap(),
            doc("a", s(""))
        );
    }

    #[test]
    fn nesting_is_bounded() {
        let ok = format!("{}{}", "<a>".repeat(MAX_DEPTH), "</a>".repeat(MAX_DEPTH));
        assert!(parse(&ok).is_ok());
        let deep = MAX_DEPTH + 5_000;
        let bad = format!("{}{}", "<a>".repeat(deep), "</a>".repeat(deep));
        assert!(err(&bad).contains("nesting"));
    }

    #[test]
    fn build_writes_the_shape_parse_reads() {
        let value = doc(
            "user",
            obj(&[
                ("@id", Value::Number(1.0)),
                ("name", s("Alice")),
                ("tag", Value::Array(vec![s("a"), s("b")])),
                ("empty", Value::Null),
            ]),
        );
        assert_eq!(
            emit(&value).unwrap(),
            "<user id=\"1\"><name>Alice</name><tag>a</tag><tag>b</tag><empty/></user>"
        );
    }

    #[test]
    fn build_escapes_so_the_text_reads_back() {
        let value = doc(
            "r",
            obj(&[
                ("@a", s("1&2 \"q\"\t\n\r<")),
                ("$text", s("<&> ]]> a\r\nb")),
            ]),
        );
        let text = emit(&value).unwrap();
        assert_eq!(
            text,
            "<r a=\"1&amp;2 &quot;q&quot;&#9;&#10;&#13;&lt;\">&lt;&amp;&gt; ]]&gt; a&#13;\nb</r>"
        );
        assert_eq!(parse(&text).unwrap(), value);
    }

    #[test]
    fn build_keeps_whitespace_at_the_edges_of_text() {
        let value = doc("r", obj(&[("$text", s(" \tkeep\n "))]));
        let text = emit(&value).unwrap();
        assert_eq!(text, "<r>&#32;&#9;keep&#10;&#32;</r>");
        assert_eq!(parse(&text).unwrap(), value);
        let spaces = doc("r", obj(&[("$text", s("   "))]));
        assert_eq!(parse(&emit(&spaces).unwrap()).unwrap(), spaces);
    }

    #[test]
    fn build_spells_numbers_as_javascript_does() {
        let value = doc(
            "r",
            obj(&[
                ("a", Value::Number(0.1)),
                ("b", Value::Number(2f64.powi(63))),
                ("c", Value::Number(1e21)),
                ("d", Value::BigInt(12_345_678_901_234_567_890)),
                ("e", Value::Bool(true)),
            ]),
        );
        assert_eq!(
            emit(&value).unwrap(),
            "<r><a>0.1</a><b>9223372036854776000</b><c>1e+21</c><d>12345678901234567890</d><e>true</e></r>"
        );
    }

    #[test]
    fn build_refuses_what_xml_cannot_spell() {
        for bad in [
            doc("r", obj(&[("n", Value::Number(f64::NAN))])),
            doc("r", obj(&[("b", Value::Bytes(vec![1]))])),
            doc("r", obj(&[("a b", s("x"))])),
            doc("r", obj(&[("@1", s("x"))])),
            doc("r", obj(&[("x", Value::Array(vec![Value::Array(vec![])]))])),
            doc("r", s("\u{0}")),
            doc("items", Value::Array(vec![s("a"), s("b")])),
            Value::Array(vec![s("a")]),
        ] {
            assert_eq!(emit(&bad).unwrap_err().kind, ErrorKind::Type, "{bad:?}");
        }
    }

    #[test]
    fn build_wraps_anything_but_a_single_element_in_root() {
        assert_eq!(emit(&s("x")).unwrap(), "<root>x</root>");
        assert_eq!(
            emit(&obj(&[("a", s("1")), ("b", s("2"))])).unwrap(),
            "<root><a>1</a><b>2</b></root>"
        );
    }

    #[test]
    fn round_trips_through_build() {
        for input in [
            "<r a=\"1&amp;2\"><x>t&lt;</x><x>u</x></r>",
            "<r><![CDATA[ keep ]]></r>",
            "<doc><a b=\"&#10;\"/><c>é ü</c></doc>",
        ] {
            fuzz_round_trip(input);
        }
    }

    fn stream(chunks: &[&str]) -> Result<Vec<Value>, Error> {
        let mut d = StreamDecoder::new();
        let mut out = Vec::new();
        for c in chunks {
            out.extend(d.push(c)?);
        }
        d.finish()?;
        Ok(out)
    }

    #[test]
    fn stream_matches_parse_at_every_split() {
        let input = "<?xml version=\"1.0\"?>\n<feed a=\"1\">\n  <item id=\"1\">a &amp; b</item>\n  <item><![CDATA[<x>]]></item>\n  <empty/>\n  <!-- c -->\n</feed>\n";
        // One record per direct child of the root, in document order — where
        // `parse` would group the two `item`s into an array.
        let expected = vec![
            obj(&[("item", obj(&[("@id", s("1")), ("$text", s("a & b"))]))]),
            obj(&[("item", obj(&[("$text", s("<x>"))]))]),
            obj(&[("empty", s(""))]),
        ];
        assert!(parse(input).is_ok());
        for split in 0..=input.len() {
            if !input.is_char_boundary(split) {
                continue;
            }
            let got = stream(&[&input[..split], &input[split..]]).unwrap();
            assert_eq!(got, expected, "split at {split}");
        }
    }

    #[test]
    fn stream_surfaces_errors_instead_of_dropping_records() {
        assert!(stream(&["<r><i>a</j><i>b</i></r>"]).is_err());
        assert!(
            stream(&["<r><i>a</i><i>b"])
                .unwrap_err()
                .message
                .contains("end of input")
        );
        assert!(
            stream(&["<r><i>&foo;</i></r>"])
                .unwrap_err()
                .message
                .contains("foo")
        );
        assert!(
            stream(&["<r/><s/>"])
                .unwrap_err()
                .message
                .contains("more than one root")
        );
        assert!(stream(&["<r></s>"]).is_err());
        assert!(stream(&["<r></r>junk"]).is_err());
        assert!(stream(&[""]).unwrap_err().message.contains("no root"));
    }

    #[test]
    fn stream_caps_an_unclosed_record() {
        let mut d = StreamDecoder::new();
        d.push("<root><item>").unwrap();
        let big = "x".repeat(MAX_STREAM_BUFFER);
        assert_eq!(d.push(&big).unwrap_err().kind, ErrorKind::Limit);
    }
}
