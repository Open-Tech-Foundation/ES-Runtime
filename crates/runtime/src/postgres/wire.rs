//! The PostgreSQL wire format (protocol 3.0): writing frontend messages and
//! framing backend ones (DECISIONS.md D147).
//!
//! Every message is `tag(1) length(4) body`, with the length counting itself.
//! The startup packet and `SSLRequest` are the two without a tag, because they
//! are sent before the server knows which protocol it is speaking.

/// Frontend message tags.
pub(super) mod front {
    pub const BIND: u8 = b'B';
    pub const CLOSE: u8 = b'C';
    pub const DESCRIBE: u8 = b'D';
    pub const EXECUTE: u8 = b'E';
    pub const PARSE: u8 = b'P';
    pub const PASSWORD: u8 = b'p';
    pub const SYNC: u8 = b'S';
    pub const TERMINATE: u8 = b'X';
}

/// Backend message tags.
pub(super) mod back {
    pub const AUTHENTICATION: u8 = b'R';
    pub const BACKEND_KEY_DATA: u8 = b'K';
    pub const BIND_COMPLETE: u8 = b'2';
    pub const CLOSE_COMPLETE: u8 = b'3';
    pub const COMMAND_COMPLETE: u8 = b'C';
    pub const DATA_ROW: u8 = b'D';
    pub const EMPTY_QUERY: u8 = b'I';
    pub const ERROR_RESPONSE: u8 = b'E';
    pub const NO_DATA: u8 = b'n';
    pub const NOTICE_RESPONSE: u8 = b'N';
    pub const NOTIFICATION: u8 = b'A';
    pub const PARAMETER_DESCRIPTION: u8 = b't';
    pub const PARAMETER_STATUS: u8 = b'S';
    pub const PARSE_COMPLETE: u8 = b'1';
    pub const PORTAL_SUSPENDED: u8 = b's';
    pub const READY_FOR_QUERY: u8 = b'Z';
    pub const ROW_DESCRIPTION: u8 = b'T';
}

/// A buffer of frontend messages, written back to back so a whole exchange
/// leaves in one write.
#[derive(Default)]
pub(super) struct Out {
    buf: Vec<u8>,
}

impl Out {
    pub(super) fn with_capacity(capacity: usize) -> Self {
        Out {
            buf: Vec::with_capacity(capacity),
        }
    }

    /// Starts a tagged message; returns where its length goes.
    pub(super) fn begin(&mut self, tag: u8) -> usize {
        self.buf.push(tag);
        self.untagged()
    }

    /// Starts a message with no tag (the startup packet, `SSLRequest`).
    pub(super) fn untagged(&mut self) -> usize {
        let at = self.buf.len();
        self.buf.extend_from_slice(&[0; 4]);
        at
    }

    /// Back-fills the length of the message started at `at`, which counts
    /// itself and not the tag.
    pub(super) fn end(&mut self, at: usize) {
        let length = (self.buf.len() - at) as i32;
        self.buf[at..at + 4].copy_from_slice(&length.to_be_bytes());
    }

    pub(super) fn u8(&mut self, value: u8) -> &mut Self {
        self.buf.push(value);
        self
    }

    pub(super) fn i16(&mut self, value: i16) -> &mut Self {
        self.buf.extend_from_slice(&value.to_be_bytes());
        self
    }

    pub(super) fn i32(&mut self, value: i32) -> &mut Self {
        self.buf.extend_from_slice(&value.to_be_bytes());
        self
    }

    pub(super) fn bytes(&mut self, value: &[u8]) -> &mut Self {
        self.buf.extend_from_slice(value);
        self
    }

    /// A NUL-terminated string.
    pub(super) fn cstr(&mut self, value: &str) -> &mut Self {
        self.buf.extend_from_slice(value.as_bytes());
        self.buf.push(0);
        self
    }

    pub(super) fn into_bytes(self) -> Vec<u8> {
        self.buf
    }
}

/// The startup packet: protocol 3.0 and the connection parameters.
pub(super) fn startup(out: &mut Out, params: &[(&str, &str)]) {
    let at = out.untagged();
    out.i32(196_608); // 3.0
    for (key, value) in params {
        if value.is_empty() {
            continue;
        }
        out.cstr(key).cstr(value);
    }
    out.u8(0);
    out.end(at);
}

/// `SSLRequest`: a length and a magic number. The server answers with a single
/// byte, because there is no agreed framing yet.
pub(super) fn ssl_request(out: &mut Out) {
    let at = out.untagged();
    out.i32(80_877_103);
    out.end(at);
}

/// A `PasswordMessage` carrying `body` as-is (cleartext, or a SASL response).
pub(super) fn password(out: &mut Out, body: &[u8]) {
    let at = out.begin(front::PASSWORD);
    out.bytes(body);
    out.end(at);
}

/// `SASLInitialResponse` for `mechanism`, with its initial client message.
pub(super) fn sasl_initial(out: &mut Out, mechanism: &str, initial: &[u8]) {
    let at = out.begin(front::PASSWORD);
    out.cstr(mechanism).i32(initial.len() as i32).bytes(initial);
    out.end(at);
}

pub(super) fn parse(out: &mut Out, name: &str, sql: &str) {
    let at = out.begin(front::PARSE);
    // No parameter type hints: the server infers them from the statement.
    out.cstr(name).cstr(sql).i16(0);
    out.end(at);
}

/// `Describe` of a prepared statement: its parameter types and row shape.
pub(super) fn describe_statement(out: &mut Out, name: &str) {
    let at = out.begin(front::DESCRIBE);
    out.u8(b'S').cstr(name);
    out.end(at);
}

/// `Close` of a prepared statement.
pub(super) fn close_statement(out: &mut Out, name: &str) {
    let at = out.begin(front::CLOSE);
    out.u8(b'S').cstr(name);
    out.end(at);
}

/// `Bind` of the unnamed portal to `statement`.
///
/// `params` is the parameter section already in wire layout — an `int16`
/// count, then per parameter an `int32` length (`-1` for NULL) and its text
/// bytes — which is how the driver hands it over, so it is copied, not parsed.
pub(super) fn bind(out: &mut Out, statement: &str, params: &[u8], formats: &[i16]) {
    let at = out.begin(front::BIND);
    out.u8(0); // the unnamed portal
    out.cstr(statement);
    out.i16(0); // every parameter in text format
    if params.is_empty() {
        out.i16(0);
    } else {
        out.bytes(params);
    }
    out.i16(formats.len() as i16);
    for &format in formats {
        out.i16(format);
    }
    out.end(at);
}

/// `Execute` of the unnamed portal, every row.
pub(super) fn execute(out: &mut Out) {
    let at = out.begin(front::EXECUTE);
    out.u8(0).i32(0);
    out.end(at);
}

pub(super) fn sync(out: &mut Out) {
    let at = out.begin(front::SYNC);
    out.end(at);
}

pub(super) fn terminate(out: &mut Out) {
    let at = out.begin(front::TERMINATE);
    out.end(at);
}

/// The bytes read off the socket, framed into messages.
///
/// A window that slides over one buffer: a message is handed out as a range
/// into it, valid until the next [`push`](Self::push), and nothing is copied
/// until a caller keeps it.
#[derive(Default)]
pub(super) struct Inbox {
    buf: Vec<u8>,
    start: usize,
}

/// One complete backend message: its tag and the range of its body (after the
/// length) in the [`Inbox`], plus the range from the length onward — which for
/// a `DataRow` *is* the row encoding D56 fixed, and is copied as-is.
#[derive(Clone, Copy)]
pub(super) struct Message {
    pub tag: u8,
    pub body: (usize, usize),
    pub framed: (usize, usize),
}

impl Inbox {
    /// Appends a chunk read off the socket.
    pub(super) fn push(&mut self, chunk: &[u8]) {
        if self.start == self.buf.len() {
            self.buf.clear();
            self.start = 0;
        } else if self.start > 0 && self.start >= self.buf.len() / 2 {
            // Slide rather than grow: a long-lived connection reads far more
            // than it ever holds at once.
            self.buf.drain(..self.start);
            self.start = 0;
        }
        self.buf.extend_from_slice(chunk);
    }

    /// How many bytes are buffered and not yet taken.
    pub(super) fn buffered(&self) -> usize {
        self.buf.len() - self.start
    }

    /// Takes one raw byte (the answer to `SSLRequest`).
    pub(super) fn byte(&mut self) -> Option<u8> {
        let byte = *self.buf.get(self.start)?;
        self.start += 1;
        Some(byte)
    }

    /// The next message if all of it has arrived, or `None`.
    pub(super) fn next(&mut self) -> Result<Option<Message>, String> {
        let available = self.buf.len() - self.start;
        if available < 5 {
            return Ok(None);
        }
        let at = self.start;
        let length = i32::from_be_bytes(self.buf[at + 1..at + 5].try_into().unwrap());
        if length < 4 {
            return Err(format!("a message declared a length of {length}"));
        }
        let length = length as usize;
        if available < 1 + length {
            return Ok(None);
        }
        self.start = at + 1 + length;
        Ok(Some(Message {
            tag: self.buf[at],
            body: (at + 5, at + 1 + length),
            framed: (at + 1, at + 1 + length),
        }))
    }

    /// The bytes of `range`, valid until the next [`push`](Self::push).
    pub(super) fn slice(&self, range: (usize, usize)) -> &[u8] {
        &self.buf[range.0..range.1]
    }
}

/// Reads fields out of a message body.
pub(super) struct Fields<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Fields<'a> {
    pub(super) fn new(bytes: &'a [u8]) -> Self {
        Fields { bytes, at: 0 }
    }

    pub(super) fn u8(&mut self) -> Result<u8, String> {
        let value = *self.bytes.get(self.at).ok_or_else(truncated)?;
        self.at += 1;
        Ok(value)
    }

    pub(super) fn i16(&mut self) -> Result<i16, String> {
        let bytes = self.bytes.get(self.at..self.at + 2).ok_or_else(truncated)?;
        self.at += 2;
        Ok(i16::from_be_bytes(bytes.try_into().unwrap()))
    }

    pub(super) fn i32(&mut self) -> Result<i32, String> {
        let bytes = self.bytes.get(self.at..self.at + 4).ok_or_else(truncated)?;
        self.at += 4;
        Ok(i32::from_be_bytes(bytes.try_into().unwrap()))
    }

    /// A NUL-terminated string.
    pub(super) fn cstr(&mut self) -> Result<String, String> {
        let rest = self.bytes.get(self.at..).ok_or_else(truncated)?;
        let end = rest.iter().position(|&b| b == 0).ok_or_else(truncated)?;
        let text = String::from_utf8_lossy(&rest[..end]).into_owned();
        self.at += end + 1;
        Ok(text)
    }

    /// Everything not read yet.
    pub(super) fn rest(&self) -> &'a [u8] {
        &self.bytes[self.at.min(self.bytes.len())..]
    }
}

fn truncated() -> String {
    "a message ended before its fields did".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_is_framed_only_once_all_of_it_has_arrived() {
        let mut out = Out::default();
        sync(&mut out);
        let bytes = out.into_bytes();
        assert_eq!(bytes, [b'S', 0, 0, 0, 4]);

        let mut inbox = Inbox::default();
        inbox.push(&[b'Z', 0, 0, 0]);
        assert!(inbox.next().unwrap().is_none());
        inbox.push(&[5, b'I']);
        let message = inbox.next().unwrap().unwrap();
        assert_eq!(message.tag, b'Z');
        assert_eq!(inbox.slice(message.body), b"I");
        assert_eq!(inbox.slice(message.framed), [0, 0, 0, 5, b'I']);
        assert_eq!(inbox.buffered(), 0);
    }

    #[test]
    fn a_length_below_four_is_refused() {
        let mut inbox = Inbox::default();
        inbox.push(&[b'Z', 0, 0, 0, 3]);
        assert!(inbox.next().is_err());
    }

    #[test]
    fn bind_copies_the_parameter_section_and_writes_formats() {
        let mut out = Out::default();
        // One parameter, "7".
        bind(&mut out, "s1", &[0, 1, 0, 0, 0, 1, b'7'], &[1]);
        let bytes = out.into_bytes();
        assert_eq!(bytes[0], b'B');
        let body = &bytes[5..];
        assert_eq!(
            body,
            [0, b's', b'1', 0, 0, 0, 0, 1, 0, 0, 0, 1, b'7', 0, 1, 0, 1]
        );
    }

    #[test]
    fn fields_read_strings_and_integers_and_refuse_to_overrun() {
        let body = [b'o', b'k', 0, 0, 0, 0, 9, 1];
        let mut fields = Fields::new(&body);
        assert_eq!(fields.cstr().unwrap(), "ok");
        assert_eq!(fields.i32().unwrap(), 9);
        assert_eq!(fields.u8().unwrap(), 1);
        assert!(fields.u8().is_err());
    }
}
