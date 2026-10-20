//! A CSV reader and writer (RFC 4180, plus the lenient extensions every parser
//! in the wild has) over the runtime's [`Value`] tree.
//!
//! Written here rather than taken from a crate for the same reason as the YAML
//! reader: it is small, the interesting cases are the malformed ones, and the
//! parser sits behind the op boundary where every byte is untrusted. Delimiters,
//! quotes and line breaks are ASCII, so the byte loop only ever splits on ASCII
//! boundaries and the borrowed input stays valid UTF-8 throughout.
//!
//! The output shape follows the mainstream. `parse` returns **arrays of fields**
//! by default (`string[][]`); `header` turns it into an array of objects keyed by
//! the first row (or by explicit names). No field is type-cast unless `cast` is
//! asked for — Papa Parse (`dynamicTyping`), csv-parse (`cast`), fast-csv and the
//! Rust `csv` crate all default to strings. Malformed input **throws** rather
//! than being collected into a result envelope, which is both what csv-parse and
//! the Rust `csv` reader do and what the other `runtime:serialization` parsers
//! already do.

use es_runtime_engine::Value;
use std::fmt;

/// Default cap on one field's length, in bytes.
pub(crate) const DEFAULT_MAX_FIELD_BYTES: usize = 16 * 1024 * 1024;
/// `Number.MAX_SAFE_INTEGER`: the largest integer every smaller one of which an
/// `f64` holds exactly.
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;
/// Default cap on the number of fields in one record.
pub(crate) const DEFAULT_MAX_COLUMNS: usize = 1 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ErrorKind {
    /// The input is not well-formed CSV.
    Syntax,
    /// An option or a value given to `build` is not representable.
    Type,
    /// The input is well-formed but exceeds a resource bound.
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

    fn limit(message: impl Into<String>) -> Error {
        Error {
            kind: ErrorKind::Limit,
            message: message.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

/// Where column names come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Header {
    /// No header: every record is an array of fields.
    None,
    /// The first record is the header; it is consumed, not returned.
    FirstRow,
    /// The given names are the header; every record is data.
    Names(Vec<String>),
}

/// Everything `parse`/`build`/`validate` understand. Built from the JS options
/// object by [`Options::from_value`].
#[derive(Debug, Clone)]
pub(crate) struct Options {
    pub delimiter: u8,
    pub quote: u8,
    /// The escape character inside a quoted field. `None` is RFC 4180 doubling
    /// (`""`); `Some(q)` with `q == quote` is the same thing spelled out.
    pub escape: Option<u8>,
    pub header: Header,
    pub trim: bool,
    pub skip_empty_lines: bool,
    pub comment: Option<u8>,
    pub cast: bool,
    pub relax_column_count: bool,
    pub max_field_bytes: usize,
    pub max_columns: usize,
    /// Strip a leading UTF-8 BOM. On by default: leaving it in corrupts the
    /// first field (and, with `header`, the first column name).
    pub bom: bool,
    /// The record separator `build` writes. Parsing accepts all three.
    pub newline: String,
}

impl Default for Options {
    fn default() -> Options {
        Options {
            delimiter: b',',
            quote: b'"',
            escape: None,
            header: Header::None,
            trim: false,
            skip_empty_lines: false,
            comment: None,
            cast: false,
            relax_column_count: false,
            max_field_bytes: DEFAULT_MAX_FIELD_BYTES,
            max_columns: DEFAULT_MAX_COLUMNS,
            bom: true,
            newline: "\n".to_string(),
        }
    }
}

fn get<'a>(obj: &'a [(String, Value)], key: &str) -> Option<&'a Value> {
    obj.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

fn get_bool(obj: &[(String, Value)], key: &str, default: bool) -> Result<bool, Error> {
    match get(obj, key) {
        None | Some(Value::Null) | Some(Value::Undefined) => Ok(default),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(Error::type_(format!(
            "CSV option {key:?} must be a boolean"
        ))),
    }
}

fn get_usize(obj: &[(String, Value)], key: &str, default: usize) -> Result<usize, Error> {
    match get(obj, key) {
        None | Some(Value::Null) | Some(Value::Undefined) => Ok(default),
        Some(Value::Number(n))
            if n.is_finite() && *n >= 0.0 && n.fract() == 0.0 && *n <= usize::MAX as f64 =>
        {
            Ok(*n as usize)
        }
        Some(_) => Err(Error::type_(format!(
            "CSV option {key:?} must be a non-negative integer"
        ))),
    }
}

/// Reads a single-byte option (a delimiter, quote, escape or comment char).
/// Multi-byte options are refused rather than silently truncated.
fn get_byte(obj: &[(String, Value)], key: &str) -> Result<Option<u8>, Error> {
    match get(obj, key) {
        None | Some(Value::Null) | Some(Value::Undefined) => Ok(None),
        Some(Value::String(s)) => match s.as_bytes() {
            [b] => Ok(Some(*b)),
            _ => Err(Error::type_(format!(
                "CSV option {key:?} must be a single byte"
            ))),
        },
        Some(_) => Err(Error::type_(format!("CSV option {key:?} must be a string"))),
    }
}

impl Options {
    /// Reads an options object. A missing or `null` object is the default set.
    pub(crate) fn from_value(value: Option<&Value>) -> Result<Options, Error> {
        let obj = match value {
            None | Some(Value::Null) | Some(Value::Undefined) => return Ok(Options::default()),
            Some(Value::Object(obj)) => obj.as_slice(),
            Some(_) => return Err(Error::type_("CSV options must be an object")),
        };

        let mut options = Options::default();
        if let Some(b) = get_byte(obj, "delimiter")? {
            options.delimiter = b;
        }
        if let Some(b) = get_byte(obj, "quote")? {
            options.quote = b;
        }
        if get(obj, "escape").is_some() {
            options.escape = get_byte(obj, "escape")?;
        }
        // Combinations that cannot be told apart while scanning: a delimiter or
        // quote that is a line break, or a delimiter equal to the quote. Reading
        // such input back is ambiguous, so an option set that produced it is
        // refused rather than parsed into something the writer would not rebuild.
        if options.delimiter == b'\n' || options.delimiter == b'\r' {
            return Err(Error::type_("CSV delimiter cannot be a line break"));
        }
        if options.quote == b'\n' || options.quote == b'\r' {
            return Err(Error::type_("CSV quote cannot be a line break"));
        }
        if options.delimiter == options.quote {
            return Err(Error::type_("CSV delimiter and quote must differ"));
        }
        options.trim = get_bool(obj, "trim", options.trim)?;
        options.skip_empty_lines = get_bool(obj, "skipEmptyLines", options.skip_empty_lines)?;
        options.cast = get_bool(obj, "cast", options.cast)?;
        options.relax_column_count = get_bool(obj, "relaxColumnCount", options.relax_column_count)?;
        options.max_field_bytes = get_usize(obj, "maxFieldBytes", options.max_field_bytes)?;
        options.max_columns = get_usize(obj, "maxColumns", options.max_columns)?;
        options.bom = get_bool(obj, "bom", options.bom)?;
        if let Some(b) = get_byte(obj, "comment")? {
            // A comment byte that is also the delimiter or the quote makes an
            // ordinary record look like a comment: `build` writes `,x` (an empty
            // first field) or `"a,b"` and `parse` drops the line. A line break
            // would turn every blank line into a comment. Refused like the
            // delimiter/quote clashes above.
            if b == options.delimiter || b == options.quote || b == b'\n' || b == b'\r' {
                return Err(Error::type_(
                    "CSV comment cannot be the delimiter, the quote or a line break",
                ));
            }
            options.comment = Some(b);
        }

        options.header = match get(obj, "header") {
            None | Some(Value::Null) | Some(Value::Undefined) | Some(Value::Bool(false)) => {
                Header::None
            }
            Some(Value::Bool(true)) => Header::FirstRow,
            Some(Value::Array(names)) => {
                let mut out = Vec::with_capacity(names.len());
                for n in names {
                    match n {
                        Value::String(s) => out.push(s.clone()),
                        _ => {
                            return Err(Error::type_(
                                "CSV option \"header\" names must be strings",
                            ));
                        }
                    }
                }
                // Named columns share the inferred header's rule: two columns of
                // the same name would key one object twice and drop a field.
                // Checked here as well as for `Header::FirstRow` in `compose`.
                check_duplicates(&out)?;
                Header::Names(out)
            }
            Some(_) => {
                return Err(Error::type_(
                    "CSV option \"header\" must be a boolean or an array of strings",
                ));
            }
        };

        options.newline = match get(obj, "newline") {
            None | Some(Value::Null) | Some(Value::Undefined) => "\n".to_string(),
            Some(Value::String(s)) if s == "\n" || s == "\r\n" || s == "\r" => s.clone(),
            Some(_) => {
                return Err(Error::type_(
                    "CSV option \"newline\" must be \"\\n\", \"\\r\\n\" or \"\\r\"",
                ));
            }
        };

        Ok(options)
    }
}

/// Parses CSV text into the value tree described in the module docs.
pub(crate) fn parse(input: &str, options: &Options) -> Result<Value, Error> {
    compose(parse_records(input, options)?, options)
}

/// Checks that `input` would parse to something.
pub(crate) fn validate(input: &str, options: &Options) -> Result<(), Error> {
    parse(input, options).map(|_| ())
}

fn push_byte(field: &mut Vec<u8>, byte: u8, options: &Options) -> Result<(), Error> {
    field.push(byte);
    if options.max_field_bytes > 0 && field.len() > options.max_field_bytes {
        return Err(Error::limit(format!(
            "CSV field exceeds {} bytes",
            options.max_field_bytes
        )));
    }
    Ok(())
}

fn end_field(
    record: &mut Vec<String>,
    field: &mut Vec<u8>,
    quoted: bool,
    options: &Options,
) -> Result<(), Error> {
    // The bytes came from a `&str`, so this cannot actually fail; the fallible
    // conversion documents that the invariant is upheld rather than assumed.
    let text = String::from_utf8(std::mem::take(field))
        .map_err(|_| Error::type_("CSV field is not valid UTF-8"))?;
    // Trimming respects an explicit quote: `"  x  "` was asked to keep its
    // spaces, so `trim` only touches fields that were not quoted.
    let text = if options.trim && !quoted {
        text.trim().to_string()
    } else {
        text
    };
    record.push(text);
    if options.max_columns > 0 && record.len() > options.max_columns {
        return Err(Error::limit(format!(
            "CSV record exceeds {} columns",
            options.max_columns
        )));
    }
    Ok(())
}

/// The byte-level scanner. Returns raw records (fields as strings), with `bom`,
/// `trim`, `skipEmptyLines` and `comment` applied but no header interpretation.
fn parse_records(input: &str, options: &Options) -> Result<Vec<Vec<String>>, Error> {
    let input = if options.bom {
        input.strip_prefix('\u{feff}').unwrap_or(input)
    } else {
        input
    };
    let bytes = input.as_bytes();

    let mut records: Vec<Vec<String>> = Vec::new();
    let mut record: Vec<String> = Vec::new();
    let mut field: Vec<u8> = Vec::new();

    let mut in_quotes = false;
    let mut field_was_quoted = false;
    // Set the instant a closing quote is seen; the only legal next bytes are a
    // delimiter, a line break or EOF. A stray character (the classic `"a"b`)
    // is an error rather than being glued onto the field.
    let mut after_quote = false;
    let mut at_field_start = true;
    let mut at_record_start = true;
    // Whether a field has begun since the last record terminator, so a trailing
    // `\n` does not manufacture an empty final record while `a,b` still does.
    let mut pending = false;

    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];

        if in_quotes {
            if b == options.quote {
                let doubling = options.escape.is_none() || options.escape == Some(options.quote);
                if doubling && bytes.get(i + 1) == Some(&options.quote) {
                    push_byte(&mut field, options.quote, options)?;
                    i += 2;
                    continue;
                }
                in_quotes = false;
                after_quote = true;
                i += 1;
                continue;
            }
            if let Some(esc) = options.escape
                && esc != options.quote
                && b == esc
                && let Some(&next) = bytes.get(i + 1)
            {
                push_byte(&mut field, next, options)?;
                i += 2;
                continue;
            }
            push_byte(&mut field, b, options)?;
            i += 1;
            continue;
        }

        // A closing quote must be followed by a delimiter, a line break or EOF
        // (or, under `trim`, blanks before one).
        if after_quote && b != options.delimiter && b != b'\n' && b != b'\r' {
            if is_trimmed_blank(b, options) {
                i += 1;
                continue;
            }
            return Err(Error::syntax(
                "unexpected character after closing quote".to_string(),
            ));
        }

        // A comment line is skipped whole; it never produces a record.
        if at_record_start && options.comment == Some(b) {
            while i < bytes.len() && bytes[i] != b'\n' && bytes[i] != b'\r' {
                i += 1;
            }
            if i < bytes.len() {
                let terminator = bytes[i];
                i += 1;
                if terminator == b'\r' && bytes.get(i) == Some(&b'\n') {
                    i += 1;
                }
            }
            field.clear();
            record.clear();
            field_was_quoted = false;
            after_quote = false;
            at_field_start = true;
            at_record_start = true;
            pending = false;
            continue;
        }

        // Under `trim`, blanks before a field's first byte are skipped rather
        // than collected, so `a, "b, c"` still opens a quoted field.
        if at_field_start && is_trimmed_blank(b, options) {
            at_record_start = false;
            pending = true;
            i += 1;
            continue;
        }

        if at_field_start && b == options.quote {
            in_quotes = true;
            field_was_quoted = true;
            at_field_start = false;
            at_record_start = false;
            pending = true;
            i += 1;
            continue;
        }

        if b == b'\n' || b == b'\r' {
            end_field(&mut record, &mut field, field_was_quoted, options)?;
            records.push(std::mem::take(&mut record));
            field_was_quoted = false;
            after_quote = false;
            at_field_start = true;
            at_record_start = true;
            pending = false;
            i += 1;
            if b == b'\r' && bytes.get(i) == Some(&b'\n') {
                i += 1;
            }
            continue;
        }

        if b == options.delimiter {
            end_field(&mut record, &mut field, field_was_quoted, options)?;
            field_was_quoted = false;
            after_quote = false;
            at_field_start = true;
            at_record_start = false;
            pending = true;
            i += 1;
            continue;
        }

        push_byte(&mut field, b, options)?;
        at_field_start = false;
        at_record_start = false;
        after_quote = false;
        pending = true;
        i += 1;
    }

    if in_quotes {
        return Err(Error::syntax("unterminated quoted field".to_string()));
    }
    if pending {
        end_field(&mut record, &mut field, field_was_quoted, options)?;
        records.push(std::mem::take(&mut record));
    }

    Ok(records)
}

fn compose(records: Vec<Vec<String>>, options: &Options) -> Result<Value, Error> {
    let records: Vec<Vec<String>> = if options.skip_empty_lines {
        records
            .into_iter()
            .filter(|r| !(r.len() == 1 && r[0].is_empty()))
            .collect()
    } else {
        records
    };

    match &options.header {
        Header::None => Ok(Value::Array(
            records
                .into_iter()
                .map(|row| row_value(row, options))
                .collect(),
        )),
        Header::FirstRow => {
            let mut iter = records.into_iter();
            let header = match iter.next() {
                Some(h) => h,
                None => return Ok(Value::Array(vec![])),
            };
            check_duplicates(&header)?;
            let rows = iter
                .map(|row| row_object(&header, row, options))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Value::Array(rows))
        }
        Header::Names(names) => {
            let rows = records
                .into_iter()
                .map(|row| row_object(names, row, options))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Value::Array(rows))
        }
    }
}

/// [`parse_records`] made resumable, for `CSV.DecoderStream`.
///
/// A chunk boundary can fall anywhere — between a `\r` and its `\n`, inside a
/// doubled quote (`""`), between the escape character and the byte it prefixes,
/// or inside a quoted field that spans records. So the scanner *defers* the few
/// decisions that need the next byte (`quote_pending`, `escape_pending`,
/// `skip_lf`) and keeps the current field and record across calls. A chunk is
/// then consumed exactly once, and feeding one input in any split yields the
/// same rows as [`parse`]; no re-scanning, so a stream stays linear.
///
/// Fields accumulate as raw bytes and become a `String` only when the field
/// ends, so a multi-byte character split across chunks reassembles instead of
/// being replaced — the JS side decodes with a streaming `TextDecoder`, but a
/// caller may hand-split a string, and this holds either way.
pub(crate) struct StreamDecoder {
    options: Options,
    field: Vec<u8>,
    record: Vec<String>,
    out: Vec<Value>,
    in_quotes: bool,
    field_was_quoted: bool,
    after_quote: bool,
    at_field_start: bool,
    at_record_start: bool,
    pending: bool,
    /// The last byte consumed was a `\r` terminator; a `\n` right after belongs
    /// to it and is dropped.
    skip_lf: bool,
    in_comment: bool,
    /// Inside quotes: a quote was seen, but whether it is doubled (an escaped
    /// quote) or closing needs the next byte.
    quote_pending: bool,
    /// Inside quotes: the escape character was seen; the next byte is literal.
    escape_pending: bool,
    /// Whether the leading-BOM window has passed.
    started: bool,
    /// The header names once known (`Header::FirstRow`).
    header: Option<Vec<String>>,
    header_settled: bool,
}

impl StreamDecoder {
    pub(crate) fn new(options: Options) -> StreamDecoder {
        let header_settled = !matches!(options.header, Header::FirstRow);
        StreamDecoder {
            options,
            field: Vec::new(),
            record: Vec::new(),
            out: Vec::new(),
            in_quotes: false,
            field_was_quoted: false,
            after_quote: false,
            at_field_start: true,
            at_record_start: true,
            pending: false,
            skip_lf: false,
            in_comment: false,
            quote_pending: false,
            escape_pending: false,
            started: false,
            header: None,
            header_settled,
        }
    }

    /// Feeds one chunk (which must be valid UTF-8) and returns the rows that
    /// completed during it. A row that is malformed is an error, and the stream
    /// is finished: the caller drops the state rather than continuing.
    pub(crate) fn push(&mut self, chunk: &str) -> Result<Vec<Value>, Error> {
        let had_bytes = !chunk.is_empty();
        // The BOM is stripped once, at the very start, like `parse`.
        let text = if !self.started && self.options.bom {
            chunk.strip_prefix('\u{feff}').unwrap_or(chunk)
        } else {
            chunk
        };
        for &b in text.as_bytes() {
            self.step(b)?;
        }
        if had_bytes {
            self.started = true;
        }
        Ok(std::mem::take(&mut self.out))
    }

    /// Flushes the last, unterminated record (if any) and rejects a quote left
    /// open, so the end of a stream is checked exactly like the end of a string.
    pub(crate) fn finish(&mut self) -> Result<Vec<Value>, Error> {
        // A dangling quote inside a quoted field is a closing one: a doubled
        // pair needs a second byte, and there is none left.
        if self.quote_pending {
            self.quote_pending = false;
            self.in_quotes = false;
            self.after_quote = true;
        }
        // A dangling escape character is literal, as it is at the end of a
        // string — which then leaves the field open, so the check below fires.
        if self.escape_pending {
            self.escape_pending = false;
            if let Some(esc) = self.options.escape {
                push_byte(&mut self.field, esc, &self.options)?;
            }
        }
        if self.in_quotes {
            return Err(Error::syntax("unterminated quoted field".to_string()));
        }
        // A comment that ran to the end of input is discarded, like any other.
        self.in_comment = false;
        if self.pending {
            end_field(
                &mut self.record,
                &mut self.field,
                self.field_was_quoted,
                &self.options,
            )?;
            self.pending = false;
            self.complete_record()?;
        }
        Ok(std::mem::take(&mut self.out))
    }

    fn reset_record(&mut self) {
        self.field.clear();
        self.record.clear();
        self.field_was_quoted = false;
        self.after_quote = false;
        self.at_field_start = true;
        self.at_record_start = true;
        self.pending = false;
    }

    /// Applies `skipEmptyLines` and the header rules to one finished record,
    /// mirroring [`compose`]: the first record may be consumed as a header
    /// (`Header::FirstRow`) and something may be emitted instead.
    fn complete_record(&mut self) -> Result<(), Error> {
        if self.options.skip_empty_lines && self.record.len() == 1 && self.record[0].is_empty() {
            self.record.clear();
            return Ok(());
        }
        let value = match &self.options.header {
            Header::None => row_value(std::mem::take(&mut self.record), &self.options),
            Header::Names(names) => {
                let row = std::mem::take(&mut self.record);
                row_object(names, row, &self.options)?
            }
            Header::FirstRow => {
                if !self.header_settled {
                    let header = std::mem::take(&mut self.record);
                    check_duplicates(&header)?;
                    self.header = Some(header);
                    self.header_settled = true;
                    return Ok(());
                }
                let row = std::mem::take(&mut self.record);
                let names = self.header.as_deref().unwrap_or(&[]);
                row_object(names, row, &self.options)?
            }
        };
        self.out.push(value);
        Ok(())
    }

    fn step(&mut self, b: u8) -> Result<(), Error> {
        let delimiter = self.options.delimiter;
        let quote = self.options.quote;
        let escape = self.options.escape;
        let comment = self.options.comment;
        let doubling = escape.is_none() || escape == Some(quote);

        if self.skip_lf {
            self.skip_lf = false;
            if b == b'\n' {
                return Ok(());
            }
        }

        if self.in_comment {
            if b == b'\n' {
                self.in_comment = false;
                self.reset_record();
            } else if b == b'\r' {
                self.in_comment = false;
                self.skip_lf = true;
                self.reset_record();
            }
            return Ok(());
        }

        if self.quote_pending {
            self.quote_pending = false;
            if doubling && b == quote {
                push_byte(&mut self.field, quote, &self.options)?;
                return Ok(());
            }
            // The quote closed the field: read `b` as the byte that follows a
            // closing quote — a delimiter, a line break, or a stray character.
            self.in_quotes = false;
            self.after_quote = true;
        } else if self.escape_pending {
            self.escape_pending = false;
            push_byte(&mut self.field, b, &self.options)?;
            return Ok(());
        } else if self.in_quotes {
            if b == quote {
                if doubling {
                    self.quote_pending = true;
                } else {
                    self.in_quotes = false;
                    self.after_quote = true;
                }
                return Ok(());
            }
            if let Some(esc) = escape
                && esc != quote
                && b == esc
            {
                self.escape_pending = true;
                return Ok(());
            }
            push_byte(&mut self.field, b, &self.options)?;
            return Ok(());
        }

        if self.after_quote && b != delimiter && b != b'\n' && b != b'\r' {
            if is_trimmed_blank(b, &self.options) {
                return Ok(());
            }
            return Err(Error::syntax(
                "unexpected character after closing quote".to_string(),
            ));
        }

        if self.at_record_start && comment == Some(b) {
            self.in_comment = true;
            self.reset_record();
            return Ok(());
        }

        if self.at_field_start && is_trimmed_blank(b, &self.options) {
            self.at_record_start = false;
            self.pending = true;
            return Ok(());
        }

        if self.at_field_start && b == quote {
            self.in_quotes = true;
            self.field_was_quoted = true;
            self.at_field_start = false;
            self.at_record_start = false;
            self.pending = true;
            return Ok(());
        }

        if b == b'\n' || b == b'\r' {
            end_field(
                &mut self.record,
                &mut self.field,
                self.field_was_quoted,
                &self.options,
            )?;
            self.complete_record()?;
            self.field_was_quoted = false;
            self.after_quote = false;
            self.at_field_start = true;
            self.at_record_start = true;
            self.pending = false;
            if b == b'\r' {
                self.skip_lf = true;
            }
            return Ok(());
        }

        if b == delimiter {
            end_field(
                &mut self.record,
                &mut self.field,
                self.field_was_quoted,
                &self.options,
            )?;
            self.field_was_quoted = false;
            self.after_quote = false;
            self.at_field_start = true;
            self.at_record_start = false;
            self.pending = true;
            return Ok(());
        }

        push_byte(&mut self.field, b, &self.options)?;
        self.at_field_start = false;
        self.at_record_start = false;
        self.after_quote = false;
        self.pending = true;
        Ok(())
    }
}

/// Whether `b` is a blank `trim` skips around a field: a space or a tab, unless
/// it is the delimiter (a TSV's tab separates fields; it is never trimmed away).
fn is_trimmed_blank(b: u8, options: &Options) -> bool {
    options.trim && (b == b' ' || b == b'\t') && b != options.delimiter && b != options.quote
}

fn check_duplicates(header: &[String]) -> Result<(), Error> {
    let mut seen = std::collections::HashSet::with_capacity(header.len());
    for name in header {
        if !seen.insert(name.as_str()) {
            return Err(Error::syntax(format!("duplicate column name {name:?}")));
        }
    }
    Ok(())
}

fn row_value(row: Vec<String>, options: &Options) -> Value {
    Value::Array(row.into_iter().map(|f| cast_field(f, options)).collect())
}

fn row_object(header: &[String], row: Vec<String>, options: &Options) -> Result<Value, Error> {
    if !options.relax_column_count && row.len() != header.len() {
        return Err(Error::syntax(format!(
            "expected {} field(s) but found {}",
            header.len(),
            row.len()
        )));
    }
    // A short row (only reachable with `relaxColumnCount`) omits the missing
    // keys; a long one has its extras dropped.
    let obj = header
        .iter()
        .zip(row)
        .map(|(k, v)| (k.clone(), cast_field(v, options)))
        .collect();
    Ok(Value::Object(obj))
}

/// Casts one field when `cast` is on. Left as a string otherwise. Only plain
/// decimal numbers and the two boolean literals are recognised — `inf`, `nan`,
/// hex and thousands separators stay strings, so a cast never invents a value
/// the source did not spell out.
fn cast_field(field: String, options: &Options) -> Value {
    if !options.cast || field.is_empty() {
        return Value::String(field);
    }
    match field.as_str() {
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        _ => match parse_decimal(&field) {
            Some(n) => Value::Number(n),
            None => Value::String(field),
        },
    }
}

fn parse_decimal(s: &str) -> Option<f64> {
    let bytes = s.as_bytes();
    let mut i = 0;
    if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
        i += 1;
    }
    let int_start = i;
    while i < bytes.len() && bytes[i].is_ascii_digit() {
        i += 1;
    }
    let mut digits = i - int_start;
    if i < bytes.len() && bytes[i] == b'.' {
        i += 1;
        let frac_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        digits += i - frac_start;
    }
    if digits == 0 {
        return None;
    }
    if i < bytes.len() && (bytes[i] == b'e' || bytes[i] == b'E') {
        i += 1;
        if i < bytes.len() && (bytes[i] == b'+' || bytes[i] == b'-') {
            i += 1;
        }
        let exp_start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == exp_start {
            return None;
        }
    }
    if i != bytes.len() {
        return None;
    }
    // An integer literal past 2^53 - 1 does not survive as an `f64`:
    // `12345678901234567890` would come back as `12345678901234567000`, an ID
    // silently changed. It stays the string it was (Papa Parse's
    // `dynamicTyping` draws the same line). A fraction or exponent was already
    // a decimal approximation, so only the integer form is held back.
    let integral = !s.bytes().any(|b| matches!(b, b'.' | b'e' | b'E'));
    if integral && s.parse::<f64>().is_ok_and(|n| n.abs() > MAX_SAFE_INTEGER) {
        return None;
    }
    // A plain decimal that overflows `f64` (`1e500`) parses to `Infinity`, which
    // is a value the source did not spell out and which `build` then refuses —
    // so keep it the string it was, as `inf`/`nan` already are.
    s.parse::<f64>().ok().filter(|n| n.is_finite())
}

fn number_to_string(n: f64) -> Result<String, Error> {
    if !n.is_finite() {
        return Err(Error::type_(
            "CSV has no representation for NaN or Infinity",
        ));
    }
    Ok(js_number_string(n))
}

/// `String(n)` for a finite `n` (ECMA-262 Number::toString, radix 10), so a
/// field reads the way the same number prints in JavaScript: `1` not `1.0`,
/// `1e+300` not three hundred zeros, `1e-7` not `0.0000001`.
fn js_number_string(n: f64) -> String {
    if n == 0.0 {
        // Covers -0, which `String` also spells "0".
        return "0".to_string();
    }
    // `{:e}` yields the shortest round-trip digits as `d[.ddd]e[-]x`; Number::
    // toString lays out those same digits by the decimal exponent.
    let sci = format!("{:e}", n.abs());
    let (mantissa, exp) = sci.split_once('e').expect("`{:e}` always has an exponent");
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i32;
    // The decimal point sits after `point` digits: value = 0.digits × 10^point.
    let point = exp.parse::<i32>().expect("`{:e}` exponent is an integer") + 1;
    let mut out = String::new();
    if n < 0.0 {
        out.push('-');
    }
    if k <= point && point <= 21 {
        out.push_str(&digits);
        out.extend(std::iter::repeat_n('0', (point - k) as usize));
    } else if 0 < point && point <= 21 {
        out.push_str(&digits[..point as usize]);
        out.push('.');
        out.push_str(&digits[point as usize..]);
    } else if -6 < point && point <= 0 {
        out.push_str("0.");
        out.extend(std::iter::repeat_n('0', (-point) as usize));
        out.push_str(&digits);
    } else {
        out.push_str(&digits[..1]);
        if k > 1 {
            out.push('.');
            out.push_str(&digits[1..]);
        }
        out.push('e');
        out.push(if point - 1 < 0 { '-' } else { '+' });
        out.push_str(&(point - 1).abs().to_string());
    }
    out
}

fn cell_to_string(value: &Value) -> Result<String, Error> {
    match value {
        // `undefined` crosses like the empty cell `null` writes: a sparse or
        // absent cell is ordinary, not an error, so neither throws.
        Value::Null | Value::Undefined => Ok(String::new()),
        Value::Bool(b) => Ok(b.to_string()),
        Value::Number(n) => number_to_string(*n),
        // A `BigInt` has an exact decimal spelling, which is what `String`
        // writes; it is how an integer past 2^53 reaches `build` intact.
        Value::BigInt(n) => Ok(n.to_string()),
        Value::String(s) => Ok(s.clone()),
        _ => Err(Error::type_("CSV cannot represent this value as a field")),
    }
}

fn push_quoted(out: &mut String, field: &str, options: &Options) {
    let quote = options.quote as char;
    let escape = options.escape.unwrap_or(options.quote) as char;
    out.push(quote);
    for ch in field.chars() {
        if ch == quote {
            if escape == quote {
                out.push(quote);
                out.push(quote);
            } else {
                out.push(escape);
                out.push(quote);
            }
        } else if ch == escape {
            // A literal escape character is doubled so it reads back as itself.
            out.push(escape);
            out.push(escape);
        } else {
            out.push(ch);
        }
    }
    out.push(quote);
}

fn write_field(out: &mut String, field: &str, options: &Options) {
    let quote = options.quote;
    // A field starting with the comment character has to be quoted: written
    // plain at the start of a record, `parse` would read the whole line as a
    // comment and drop it, so `build` would not round-trip through `parse`.
    let starts_with_comment = options
        .comment
        .is_some_and(|c| field.as_bytes().first() == Some(&c));
    let needs_quote = starts_with_comment
        || field
            .bytes()
            .any(|b| b == options.delimiter || b == quote || b == b'\n' || b == b'\r');
    if needs_quote {
        push_quoted(out, field, options);
    } else {
        out.push_str(field);
    }
}

/// Writes the header row (the column names), then the record separator.
fn write_header_row(out: &mut String, columns: &[String], options: &Options) {
    for (idx, name) in columns.iter().enumerate() {
        if idx > 0 {
            out.push(options.delimiter as char);
        }
        write_field(out, name, options);
    }
    out.push_str(&options.newline);
}

/// Writes one array-row record (fields verbatim), then the record separator.
fn write_array_record(out: &mut String, fields: &[Value], options: &Options) -> Result<(), Error> {
    for (idx, field) in fields.iter().enumerate() {
        if idx > 0 {
            out.push(options.delimiter as char);
        }
        let cell = cell_to_string(field)?;
        write_field(out, &cell, options);
    }
    out.push_str(&options.newline);
    Ok(())
}

/// Writes one object-row record keyed by `columns`, then the record separator.
/// A key missing from the object is written empty; extras are dropped.
fn write_object_record(
    out: &mut String,
    obj: &[(String, Value)],
    columns: &[String],
    options: &Options,
) -> Result<(), Error> {
    for (idx, name) in columns.iter().enumerate() {
        if idx > 0 {
            out.push(options.delimiter as char);
        }
        let cell = match obj.iter().find(|(k, _)| k == name) {
            Some((_, v)) => cell_to_string(v)?,
            None => String::new(),
        };
        write_field(out, &cell, options);
    }
    out.push_str(&options.newline);
    Ok(())
}

/// Writes an array of rows as CSV text. Array rows are fields verbatim; object
/// rows are keyed by `header` (or by the first object's keys when none is given).
pub(crate) fn emit(value: &Value, options: &Options) -> Result<String, Error> {
    let rows = match value {
        Value::Array(rows) => rows.as_slice(),
        _ => return Err(Error::type_("CSV build expects an array of rows")),
    };

    let mut out = String::new();

    if matches!(rows.first(), Some(Value::Object(_))) {
        let columns: Vec<String> = match &options.header {
            Header::Names(names) => names.clone(),
            // With no explicit names the first object's keys are the header, so
            // the common `build(arrayOfObjects)` round-trips through `parse`
            // with `header: true`.
            _ => match rows.first() {
                Some(Value::Object(obj)) => obj.iter().map(|(k, _)| k.clone()).collect(),
                _ => Vec::new(),
            },
        };
        if !columns.is_empty() {
            write_header_row(&mut out, &columns, options);
        }
        for row in rows {
            let obj = match row {
                Value::Object(obj) => obj,
                _ => {
                    return Err(Error::type_(
                        "CSV build cannot mix object rows with array rows",
                    ));
                }
            };
            write_object_record(&mut out, obj, &columns, options)?;
        }
        return Ok(out);
    }

    for row in rows {
        let fields = match row {
            Value::Array(fields) => fields,
            _ => {
                return Err(Error::type_(
                    "CSV build expects every row to be an array or every row to be an object",
                ));
            }
        };
        write_array_record(&mut out, fields, options)?;
    }
    Ok(out)
}

/// One row's worth of [`emit`], made resumable for `CSV.EncoderStream`.
///
/// The first row fixes the shape, exactly as `emit` decides it from the first
/// row of the whole array: an array row is written verbatim, an object row is
/// keyed by `header` (or by the first object's keys) after writing the header
/// line. A later row of the other shape is the same `TypeError` `emit` raises.
pub(crate) struct StreamEncoder {
    options: Options,
    mode: EncoderMode,
    columns: Vec<String>,
}

enum EncoderMode {
    Unknown,
    Array,
    Object,
}

impl StreamEncoder {
    pub(crate) fn new(options: Options) -> StreamEncoder {
        StreamEncoder {
            options,
            mode: EncoderMode::Unknown,
            columns: Vec::new(),
        }
    }

    /// Writes one row and returns its text (which always ends in the record
    /// separator).
    pub(crate) fn push(&mut self, row: &Value) -> Result<String, Error> {
        let mut out = String::new();
        match self.mode {
            EncoderMode::Unknown => match row {
                Value::Array(fields) => {
                    self.mode = EncoderMode::Array;
                    write_array_record(&mut out, fields, &self.options)?;
                }
                Value::Object(obj) => {
                    self.mode = EncoderMode::Object;
                    self.columns = match &self.options.header {
                        Header::Names(names) => names.clone(),
                        _ => obj.iter().map(|(k, _)| k.clone()).collect(),
                    };
                    if !self.columns.is_empty() {
                        write_header_row(&mut out, &self.columns, &self.options);
                    }
                    write_object_record(&mut out, obj, &self.columns, &self.options)?;
                }
                _ => {
                    return Err(Error::type_(
                        "CSV build expects every row to be an array or an object",
                    ));
                }
            },
            EncoderMode::Array => match row {
                Value::Array(fields) => write_array_record(&mut out, fields, &self.options)?,
                _ => {
                    return Err(Error::type_(
                        "CSV build cannot mix object rows with array rows",
                    ));
                }
            },
            EncoderMode::Object => match row {
                Value::Object(obj) => {
                    write_object_record(&mut out, obj, &self.columns, &self.options)?
                }
                _ => {
                    return Err(Error::type_(
                        "CSV build cannot mix array rows with object rows",
                    ));
                }
            },
        }
        Ok(out)
    }
}

/// Fuzz entry: parse arbitrary text and, when it parses, write it back and read
/// it again. A reader and writer that disagree would turn a value into text the
/// reader rejects.
#[cfg(any(test, feature = "fuzzing"))]
pub(crate) fn fuzz_round_trip(input: &str) {
    let options = Options::default();
    if let Ok(value) = parse(input, &options)
        && let Ok(text) = emit(&value, &options)
    {
        let _ = parse(&text, &options);
    }
}

/// Fuzz entry: feed arbitrary text to the streaming decoder a byte at a time and
/// check it agrees with [`parse`] wherever both succeed. A chunk boundary must
/// be invisible — the one property the whole state machine exists to hold.
#[cfg(any(test, feature = "fuzzing"))]
pub(crate) fn fuzz_stream_decode(input: &str) {
    let options = Options::default();
    let mut decoder = StreamDecoder::new(options.clone());
    let mut rows = Vec::new();
    for ch in input.chars() {
        // One character at a time is the worst case for a boundary: every `\r\n`,
        // every doubled quote and every escape sequence is split across pushes.
        let mut buf = [0u8; 4];
        match decoder.push(ch.encode_utf8(&mut buf)) {
            Ok(emitted) => rows.extend(emitted),
            Err(_) => return,
        }
    }
    let Ok(emitted) = decoder.finish() else {
        return;
    };
    rows.extend(emitted);
    if let Ok(expected) = parse(input, &options) {
        debug_assert_eq!(Value::Array(rows), expected, "input {input:?}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_default(input: &str) -> Value {
        parse(input, &Options::default()).expect("should parse")
    }

    fn opts(value: Value) -> Options {
        Options::from_value(Some(&value)).expect("options should build")
    }

    fn obj(pairs: &[(&str, Value)]) -> Value {
        Value::Object(
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
        )
    }

    fn strings(row: &[&str]) -> Value {
        Value::Array(row.iter().map(|s| Value::String(s.to_string())).collect())
    }

    #[test]
    fn basic_rows_are_arrays_of_strings() {
        assert_eq!(
            parse_default("a,b\n1,2\n"),
            Value::Array(vec![strings(&["a", "b"]), strings(&["1", "2"])])
        );
    }

    #[test]
    fn missing_final_newline_still_yields_the_last_row() {
        assert_eq!(
            parse_default("a,b\n1,2"),
            Value::Array(vec![strings(&["a", "b"]), strings(&["1", "2"])])
        );
    }

    #[test]
    fn empty_input_is_an_empty_array() {
        assert_eq!(parse_default(""), Value::Array(vec![]));
        assert_eq!(parse_default("\n"), Value::Array(vec![strings(&[""])]));
    }

    #[test]
    fn quoted_field_keeps_delimiter_and_newline() {
        assert_eq!(
            parse_default("\"a,b\",\"c\nd\"\n"),
            Value::Array(vec![strings(&["a,b", "c\nd"])])
        );
    }

    #[test]
    fn doubled_quotes_unescape() {
        assert_eq!(
            parse_default("\"say \"\"hi\"\"\"\n"),
            Value::Array(vec![strings(&["say \"hi\""])])
        );
    }

    #[test]
    fn crlf_and_cr_terminate_records() {
        assert_eq!(
            parse_default("a,b\r\n1,2\r\n"),
            Value::Array(vec![strings(&["a", "b"]), strings(&["1", "2"])])
        );
        assert_eq!(
            parse_default("a,b\r1,2"),
            Value::Array(vec![strings(&["a", "b"]), strings(&["1", "2"])])
        );
    }

    #[test]
    fn trailing_delimiter_yields_a_trailing_empty_field() {
        assert_eq!(
            parse_default("a,\n"),
            Value::Array(vec![strings(&["a", ""])])
        );
    }

    #[test]
    fn unterminated_quote_is_an_error() {
        let err = parse("\"a,b", &Options::default()).expect_err("must fail");
        assert_eq!(err.kind, ErrorKind::Syntax);
    }

    #[test]
    fn stray_character_after_closing_quote_is_an_error() {
        let err = parse("\"a\"b,c\n", &Options::default()).expect_err("must fail");
        assert_eq!(err.kind, ErrorKind::Syntax);
    }

    #[test]
    fn header_true_keys_objects_from_the_first_row() {
        let options = opts(obj(&[("header", Value::Bool(true))]));
        assert_eq!(
            parse("name,age\nAlice,30\n", &options).unwrap(),
            Value::Array(vec![obj(&[
                ("name", Value::String("Alice".into())),
                ("age", Value::String("30".into()))
            ])])
        );
    }

    #[test]
    fn duplicate_header_names_are_rejected() {
        let options = opts(obj(&[("header", Value::Bool(true))]));
        let err = parse("a,a\n1,2\n", &options).expect_err("must fail");
        assert_eq!(err.kind, ErrorKind::Syntax);
    }

    #[test]
    fn explicit_header_names_leave_every_row_as_data() {
        let options = opts(obj(&[(
            "header",
            Value::Array(vec![Value::String("x".into()), Value::String("y".into())]),
        )]));
        assert_eq!(
            parse("1,2\n", &options).unwrap(),
            Value::Array(vec![obj(&[
                ("x", Value::String("1".into())),
                ("y", Value::String("2".into()))
            ])])
        );
    }

    #[test]
    fn ragged_rows_are_rejected_unless_relaxed() {
        let strict = opts(obj(&[("header", Value::Bool(true))]));
        assert!(parse("a,b\n1\n", &strict).is_err());

        let relaxed = opts(obj(&[
            ("header", Value::Bool(true)),
            ("relaxColumnCount", Value::Bool(true)),
        ]));
        assert_eq!(
            parse("a,b\n1\n", &relaxed).unwrap(),
            Value::Array(vec![obj(&[("a", Value::String("1".into()))])])
        );
    }

    #[test]
    fn trim_applies_to_unquoted_fields_only() {
        let options = opts(obj(&[("trim", Value::Bool(true))]));
        assert_eq!(
            parse("  a  ,\"  b  \"\n", &options).unwrap(),
            Value::Array(vec![strings(&["a", "  b  "])])
        );
    }

    #[test]
    fn skip_empty_lines_drops_blank_records() {
        let options = opts(obj(&[("skipEmptyLines", Value::Bool(true))]));
        assert_eq!(
            parse("a\n\nb\n", &options).unwrap(),
            Value::Array(vec![strings(&["a"]), strings(&["b"])])
        );
    }

    #[test]
    fn comment_lines_are_skipped_whole() {
        let options = opts(obj(&[("comment", Value::String("#".into()))]));
        assert_eq!(
            parse("# note\na,b\n# another\n1,2\n", &options).unwrap(),
            Value::Array(vec![strings(&["a", "b"]), strings(&["1", "2"])])
        );
    }

    #[test]
    fn a_leading_bom_is_stripped_by_default_and_kept_on_request() {
        assert_eq!(
            parse_default("\u{feff}a,b\n"),
            Value::Array(vec![strings(&["a", "b"])])
        );
        let keep = opts(obj(&[("bom", Value::Bool(false))]));
        assert_eq!(
            parse("\u{feff}a,b\n", &keep).unwrap(),
            Value::Array(vec![strings(&["\u{feff}a", "b"])])
        );
    }

    #[test]
    fn cast_converts_plain_numbers_and_booleans_only() {
        let options = opts(obj(&[("cast", Value::Bool(true))]));
        assert_eq!(
            parse("1,-2,3.5,1e3,true,false,inf\n", &options).unwrap(),
            Value::Array(vec![Value::Array(vec![
                Value::Number(1.0),
                Value::Number(-2.0),
                Value::Number(3.5),
                Value::Number(1000.0),
                Value::Bool(true),
                Value::Bool(false),
                // Not a plain decimal, so it stays the string it was.
                Value::String("inf".into()),
            ])])
        );
    }

    #[test]
    fn cast_leaves_empties_as_strings() {
        let options = opts(obj(&[("cast", Value::Bool(true))]));
        assert_eq!(
            parse("007,\n", &options).unwrap(),
            Value::Array(vec![Value::Array(vec![
                // Leading zeros are still a plain decimal, so they cast.
                Value::Number(7.0),
                // An empty field is never a number; it stays "".
                Value::String("".into()),
            ])])
        );
    }

    #[test]
    fn custom_delimiter_and_quote() {
        let options = opts(obj(&[
            ("delimiter", Value::String(";".into())),
            ("quote", Value::String("'".into())),
        ]));
        assert_eq!(
            parse("a;'b;c'\n", &options).unwrap(),
            Value::Array(vec![strings(&["a", "b;c"])])
        );
    }

    #[test]
    fn multi_byte_delimiter_is_rejected() {
        let err = Options::from_value(Some(&obj(&[("delimiter", Value::String("::".into()))])))
            .expect_err("must fail");
        assert_eq!(err.kind, ErrorKind::Type);
    }

    #[test]
    fn column_and_field_limits_fire() {
        let many = opts(obj(&[("maxColumns", Value::Number(2.0))]));
        assert_eq!(
            parse("a,b,c\n", &many).expect_err("must fail").kind,
            ErrorKind::Limit
        );
        let small = opts(obj(&[("maxFieldBytes", Value::Number(2.0))]));
        assert_eq!(
            parse("abcd\n", &small).expect_err("must fail").kind,
            ErrorKind::Limit
        );
    }

    #[test]
    fn build_quotes_only_when_needed() {
        let out = emit(
            &Value::Array(vec![strings(&["a", "b,c", "d\"e", "f\ng"])]),
            &Options::default(),
        )
        .unwrap();
        assert_eq!(out, "a,\"b,c\",\"d\"\"e\",\"f\ng\"\n");
    }

    #[test]
    fn build_writes_objects_with_an_inferred_header() {
        let out = emit(
            &Value::Array(vec![obj(&[
                ("name", Value::String("Alice".into())),
                ("age", Value::Number(30.0)),
            ])]),
            &Options::default(),
        )
        .unwrap();
        assert_eq!(out, "name,age\nAlice,30\n");
    }

    #[test]
    fn build_and_parse_round_trip() {
        let options = opts(obj(&[("header", Value::Bool(true))]));
        let source = "name,note\nAlice,\"hi, there\"\nBob,\"say \"\"hi\"\"\"\n";
        let value = parse(source, &options).unwrap();
        let rebuilt = emit(&value, &options).unwrap();
        assert_eq!(parse(&rebuilt, &options).unwrap(), value);
    }

    #[test]
    fn build_refuses_nan_and_nested_values() {
        let nan = Value::Array(vec![Value::Array(vec![Value::Number(f64::NAN)])]);
        assert_eq!(
            emit(&nan, &Options::default()).unwrap_err().kind,
            ErrorKind::Type
        );
        let nested = Value::Array(vec![Value::Array(vec![strings(&["x"])])]);
        assert_eq!(
            emit(&nested, &Options::default()).unwrap_err().kind,
            ErrorKind::Type
        );
    }

    #[test]
    fn custom_newline_is_used_by_build() {
        let options = opts(obj(&[("newline", Value::String("\r\n".into()))]));
        let out = emit(&Value::Array(vec![strings(&["a", "b"])]), &options).unwrap();
        assert_eq!(out, "a,b\r\n");
    }

    #[test]
    fn fuzz_entry_does_not_panic() {
        for input in [
            "",
            "\"",
            "a,b\n",
            "\u{feff}\"x\ny\"\r\n",
            "a,\"b\r\nc\",d\n",
            "#c\nx\r\n",
        ] {
            fuzz_round_trip(input);
            fuzz_stream_decode(input);
        }
    }

    #[test]
    fn build_quotes_a_field_that_starts_with_the_comment_character() {
        let options = opts(obj(&[("comment", Value::String("#".into()))]));
        let value = Value::Array(vec![strings(&["#note", "val"])]);
        let out = emit(&value, &options).unwrap();
        assert_eq!(out, "\"#note\",val\n");
        // The whole point: a round trip keeps the row rather than dropping it as
        // a comment. Without the quote this parsed back to `[]`.
        assert_eq!(parse(&out, &options).unwrap(), value);
    }

    #[test]
    fn build_treats_undefined_as_an_empty_field() {
        let out = emit(
            &Value::Array(vec![Value::Array(vec![
                Value::String("a".into()),
                Value::Undefined,
            ])]),
            &Options::default(),
        )
        .unwrap();
        assert_eq!(out, "a,\n");
    }

    #[test]
    fn explicit_header_rejects_duplicate_names() {
        // The inferred header already rejects these; a named one must too, or
        // one of the two keyed fields is silently dropped.
        let err = Options::from_value(Some(&obj(&[(
            "header",
            Value::Array(vec![Value::String("a".into()), Value::String("a".into())]),
        )])))
        .expect_err("must fail");
        assert_eq!(err.kind, ErrorKind::Syntax);
    }

    #[test]
    fn cast_keeps_a_number_that_overflows_f64_as_a_string() {
        let options = opts(obj(&[("cast", Value::Bool(true))]));
        assert_eq!(
            parse("1e500\n", &options).unwrap(),
            Value::Array(vec![Value::Array(vec![Value::String("1e500".into())])])
        );
        // A finite one still casts.
        assert_eq!(
            parse("1e308\n", &options).unwrap(),
            Value::Array(vec![Value::Array(vec![Value::Number(1e308)])])
        );
    }

    #[test]
    fn number_to_string_reads_back_exactly_at_i64_bounds() {
        // Past ±2^63 an `i64` cast would saturate and change the number; the
        // JavaScript spelling has no such bound and parses back to the same f64.
        for n in [2f64.powi(63), i64::MIN as f64, 42.0, -0.5] {
            let written = number_to_string(n).unwrap();
            assert_eq!(written.parse::<f64>().unwrap(), n, "{written}");
        }
        assert_ne!(
            number_to_string(2f64.powi(63)).unwrap(),
            "9223372036854775807"
        );
    }

    #[test]
    fn conflicting_delimiter_and_quote_are_rejected() {
        for bad in [
            obj(&[("delimiter", Value::String("\n".into()))]),
            obj(&[("delimiter", Value::String("\r".into()))]),
            obj(&[("quote", Value::String("\n".into()))]),
            obj(&[("quote", Value::String("\r".into()))]),
            obj(&[("delimiter", Value::String("\"".into()))]),
        ] {
            let err = Options::from_value(Some(&bad)).expect_err("must fail");
            assert_eq!(err.kind, ErrorKind::Type);
        }
    }

    fn decode_chunks(chunks: &[&str], options: &Options) -> Result<Vec<Value>, Error> {
        let mut decoder = StreamDecoder::new(options.clone());
        let mut rows = Vec::new();
        for chunk in chunks {
            rows.extend(decoder.push(chunk)?);
        }
        rows.extend(decoder.finish()?);
        Ok(rows)
    }

    /// The whole contract of the streaming scanner: feeding any input in any
    /// split must equal the one-shot `parse`. This is what a `TransformStream`
    /// that re-sends on a chunk boundary silently gets wrong.
    #[test]
    fn stream_decoder_matches_parse_at_every_split() {
        let cases: &[&str] = &[
            "a,b\n1,2\n",
            "a,b\r\n1,2\r\n",
            "name,note\r\nAlice,\"hi, there\"\r\nBob,\"say \"\"hi\"\"\"\r\n",
            // A quoted field whose content spans the `\r\n` terminator.
            "\"line1\r\nline2\",x\r\n",
            // Bare CR, no final newline, empty input, lone newline.
            "a,b\r1,2",
            "",
            "\n",
            // A comment line and a leading BOM.
            "# a comment\na,b\n",
            "\u{feff}a,b\n",
        ];
        let options = Options::default();
        for input in cases {
            let expected = parse(input, &options).unwrap();
            for split in 0..=input.len() {
                if !input.is_char_boundary(split) {
                    continue;
                }
                let got = Value::Array(
                    decode_chunks(&[&input[..split], &input[split..]], &options).unwrap(),
                );
                assert_eq!(got, expected, "input {input:?} split at {split}");
            }
        }
    }

    #[test]
    fn stream_decoder_applies_header_across_chunks() {
        let options = opts(obj(&[("header", Value::Bool(true))]));
        let input = "name,note\r\nAlice,\"hi, there\"\r\nBob,\"say \"\"hi\"\"\"\r\n";
        let expected = parse(input, &options).unwrap();
        for split in 0..=input.len() {
            if !input.is_char_boundary(split) {
                continue;
            }
            let got =
                Value::Array(decode_chunks(&[&input[..split], &input[split..]], &options).unwrap());
            assert_eq!(got, expected, "split at {split}");
        }
    }

    #[test]
    fn stream_decoder_honours_a_custom_escape_across_chunks() {
        let options = opts(obj(&[
            ("quote", Value::String("'".into())),
            ("escape", Value::String("\\".into())),
        ]));
        let input = "'a\\'b',c\n";
        let expected = parse(input, &options).unwrap();
        for split in 0..=input.len() {
            if !input.is_char_boundary(split) {
                continue;
            }
            let got =
                Value::Array(decode_chunks(&[&input[..split], &input[split..]], &options).unwrap());
            assert_eq!(got, expected, "split at {split}");
        }
    }

    #[test]
    fn stream_decoder_rejects_an_unterminated_quote_at_finish() {
        let mut decoder = StreamDecoder::new(Options::default());
        assert!(decoder.push("\"a,b").unwrap().is_empty());
        assert_eq!(decoder.finish().unwrap_err().kind, ErrorKind::Syntax);
    }

    #[test]
    fn stream_encoder_matches_emit_for_object_rows() {
        let options = Options::default();
        let rows = Value::Array(vec![
            obj(&[
                ("name", Value::String("Alice".into())),
                ("age", Value::Number(30.0)),
            ]),
            obj(&[
                ("name", Value::String("Bob".into())),
                ("age", Value::Number(41.0)),
            ]),
        ]);
        let expected = emit(&rows, &options).unwrap();
        let Value::Array(rows) = rows else {
            unreachable!()
        };
        let mut encoder = StreamEncoder::new(options);
        let mut out = String::new();
        for row in &rows {
            out.push_str(&encoder.push(row).unwrap());
        }
        assert_eq!(out, expected);
    }

    #[test]
    fn comment_cannot_clash_with_delimiter_quote_or_line_break() {
        // Each of these would make `build` write a record `parse` reads as a
        // comment and drops.
        for (delimiter, comment) in [
            (",", ","),
            (",", "\""),
            (",", "\n"),
            (",", "\r"),
            ("#", "#"),
        ] {
            let err = Options::from_value(Some(&obj(&[
                ("delimiter", Value::String(delimiter.into())),
                ("comment", Value::String(comment.into())),
            ])))
            .expect_err("must fail");
            assert_eq!(err.kind, ErrorKind::Type, "comment {comment:?}");
        }
    }

    #[test]
    fn trim_skips_blanks_around_a_quoted_field() {
        let options = opts(obj(&[("trim", Value::Bool(true))]));
        let input = "a, \"b, c\" ,\t\"d\"\t\n";
        let expected = Value::Array(vec![strings(&["a", "b, c", "d"])]);
        assert_eq!(parse(input, &options).unwrap(), expected);
        for split in 0..=input.len() {
            let got =
                Value::Array(decode_chunks(&[&input[..split], &input[split..]], &options).unwrap());
            assert_eq!(got, expected, "split at {split}");
        }
        // Without `trim` the blank is data, and a quote after it is a literal.
        assert_eq!(
            parse_default("a, \"b\"\n"),
            Value::Array(vec![strings(&["a", " \"b\""])])
        );
        // A stray non-blank after the closing quote is still an error.
        assert_eq!(
            parse("\"a\" x\n", &options).unwrap_err().kind,
            ErrorKind::Syntax
        );
    }

    #[test]
    fn trim_never_eats_a_tab_delimiter() {
        let options = opts(obj(&[
            ("trim", Value::Bool(true)),
            ("delimiter", Value::String("\t".into())),
        ]));
        assert_eq!(
            parse("\t \"x\" \ty\n", &options).unwrap(),
            Value::Array(vec![strings(&["", "x", "y"])])
        );
    }

    #[test]
    fn cast_keeps_an_integer_past_the_safe_range_as_a_string() {
        let options = opts(obj(&[("cast", Value::Bool(true))]));
        assert_eq!(
            parse(
                "12345678901234567890,9007199254740992,9007199254740991,-9007199254740993,1e20\n",
                &options
            )
            .unwrap(),
            Value::Array(vec![Value::Array(vec![
                Value::String("12345678901234567890".into()),
                Value::String("9007199254740992".into()),
                Value::Number(9_007_199_254_740_991.0),
                Value::String("-9007199254740993".into()),
                // An exponent form was never exact digits; it still casts.
                Value::Number(1e20),
            ])])
        );
    }

    #[test]
    fn build_writes_numbers_as_javascript_prints_them() {
        for (n, text) in [
            (1.0, "1"),
            (-0.0, "0"),
            (0.1, "0.1"),
            (-1.5, "-1.5"),
            (123456.789, "123456.789"),
            (1e21, "1e+21"),
            (1e20, "100000000000000000000"),
            (1e300, "1e+300"),
            (1.5e-7, "1.5e-7"),
            (1e-6, "0.000001"),
            (0.000123, "0.000123"),
            (2f64.powi(64), "18446744073709552000"),
            (f64::MAX, "1.7976931348623157e+308"),
            (5e-324, "5e-324"),
        ] {
            assert_eq!(js_number_string(n), text, "{n:e}");
        }
    }

    #[test]
    fn build_writes_a_bigint_exactly() {
        let out = emit(
            &Value::Array(vec![Value::Array(vec![Value::BigInt(
                12_345_678_901_234_567_890,
            )])]),
            &Options::default(),
        )
        .unwrap();
        assert_eq!(out, "12345678901234567890\n");
    }

    #[test]
    fn stream_encoder_refuses_to_mix_row_shapes() {
        let options = Options::default();
        let mut encoder = StreamEncoder::new(options);
        assert_eq!(
            encoder.push(&strings(&["a", "b,c"])).unwrap(),
            "a,\"b,c\"\n"
        );
        // The first row fixed the shape; an object now is the same refusal
        // `emit` makes for a mixed array.
        assert_eq!(
            encoder
                .push(&obj(&[("x", Value::Number(1.0))]))
                .unwrap_err()
                .kind,
            ErrorKind::Type
        );
    }
}
