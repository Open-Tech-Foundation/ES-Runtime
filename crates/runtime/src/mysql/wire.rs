//! MySQL packets (DECISIONS.md D147): a three-byte little-endian payload
//! length, a one-byte sequence id, then the payload.
//!
//! The sequence id counts packets within one command and restarts at zero with
//! the next. A payload of exactly 2^24 - 1 bytes means "continued in the next
//! packet", which is how a value larger than 16 MiB crosses at all.

/// The largest payload one packet carries; one this long continues.
pub(super) const MAX_PAYLOAD: usize = 0xff_ffff;

/// A payload being built, framed into packets by [`Out::finish`].
pub(super) struct Out {
    buf: Vec<u8>,
}

impl Out {
    pub(super) fn new(capacity: usize) -> Out {
        let mut buf = Vec::with_capacity(capacity + 4);
        buf.extend_from_slice(&[0; 4]); // the first packet's header, written in place
        Out { buf }
    }

    pub(super) fn u8(&mut self, value: u8) -> &mut Self {
        self.buf.push(value);
        self
    }

    pub(super) fn u16(&mut self, value: u16) -> &mut Self {
        self.buf.extend_from_slice(&value.to_le_bytes());
        self
    }

    pub(super) fn u32(&mut self, value: u32) -> &mut Self {
        self.buf.extend_from_slice(&value.to_le_bytes());
        self
    }

    pub(super) fn zeros(&mut self, n: usize) -> &mut Self {
        self.buf.resize(self.buf.len() + n, 0);
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

    /// A length-encoded integer.
    pub(super) fn lenenc(&mut self, value: u64) -> &mut Self {
        if value < 0xfb {
            self.u8(value as u8)
        } else if value <= 0xffff {
            self.u8(0xfc).u16(value as u16)
        } else if value <= 0xff_ffff {
            self.u8(0xfd).u16(value as u16).u8((value >> 16) as u8)
        } else {
            self.u8(0xfe);
            self.buf.extend_from_slice(&value.to_le_bytes());
            self
        }
    }

    pub(super) fn lenenc_bytes(&mut self, value: &[u8]) -> &mut Self {
        self.lenenc(value.len() as u64).bytes(value)
    }

    /// The payload framed as packets from sequence id `seq`. One packet in the
    /// common case, its header written into the room kept at the front; a
    /// payload of 16 MiB or more is split, and one that is an exact multiple of
    /// the limit ends with an empty packet, which is how the reader knows.
    pub(super) fn finish(mut self, seq: u8) -> Vec<u8> {
        let length = self.buf.len() - 4;
        if length < MAX_PAYLOAD {
            header(&mut self.buf[0..4], length, seq);
            return self.buf;
        }
        let payload = &self.buf[4..];
        let parts = length / MAX_PAYLOAD + 1;
        let mut out = Vec::with_capacity(length + parts * 4);
        for (i, chunk) in payload
            .chunks(MAX_PAYLOAD)
            .chain(std::iter::once(&[][..]).filter(|_| length.is_multiple_of(MAX_PAYLOAD)))
            .enumerate()
        {
            let mut head = [0u8; 4];
            header(&mut head, chunk.len(), seq.wrapping_add(i as u8));
            out.extend_from_slice(&head);
            out.extend_from_slice(chunk);
        }
        out
    }
}

fn header(out: &mut [u8], length: usize, seq: u8) {
    out[0] = length as u8;
    out[1] = (length >> 8) as u8;
    out[2] = (length >> 16) as u8;
    out[3] = seq;
}

/// The bytes read off the socket, framed into packets.
#[derive(Default)]
pub(super) struct Inbox {
    buf: Vec<u8>,
    start: usize,
    /// A payload continued across packets, joined into a buffer of its own.
    joined: Vec<u8>,
    /// The sequence id of the last packet taken: the next one written follows.
    pub seq: u8,
}

/// Where the next payload is: a range of the [`Inbox`]'s buffer, or the joined
/// buffer for a continued one.
#[derive(Clone, Copy)]
pub(super) enum Payload {
    Span(usize, usize),
    Joined,
}

impl Inbox {
    pub(super) fn push(&mut self, chunk: &[u8]) {
        if self.start == self.buf.len() {
            self.buf.clear();
            self.start = 0;
        } else if self.start > 0 && self.start >= self.buf.len() / 2 {
            self.buf.drain(..self.start);
            self.start = 0;
        }
        self.buf.extend_from_slice(chunk);
    }

    pub(super) fn buffered(&self) -> usize {
        self.buf.len() - self.start
    }

    /// The payload length of the packet at `at` if all of it is here.
    fn complete(&self, at: usize) -> Option<usize> {
        if self.buf.len() < at + 4 {
            return None;
        }
        let length = self.buf[at] as usize
            | (self.buf[at + 1] as usize) << 8
            | (self.buf[at + 2] as usize) << 16;
        (self.buf.len() - at - 4 >= length).then_some(length)
    }

    /// The next payload if it has arrived whole.
    pub(super) fn next(&mut self) -> Option<Payload> {
        let length = self.complete(self.start)?;
        if length == MAX_PAYLOAD {
            return self.join();
        }
        let at = self.start;
        self.seq = self.buf[at + 3];
        self.start = at + 4 + length;
        Some(Payload::Span(at + 4, at + 4 + length))
    }

    fn join(&mut self) -> Option<Payload> {
        let mut at = self.start;
        loop {
            let length = self.complete(at)?;
            at += 4 + length;
            if length < MAX_PAYLOAD {
                break;
            }
        }
        self.joined.clear();
        while self.start < at {
            let length = self.complete(self.start).expect("checked above");
            self.joined
                .extend_from_slice(&self.buf[self.start + 4..self.start + 4 + length]);
            self.seq = self.buf[self.start + 3];
            self.start += 4 + length;
        }
        Some(Payload::Joined)
    }

    /// The bytes of a payload, valid until the next [`push`](Self::push).
    pub(super) fn bytes(&self, payload: Payload) -> &[u8] {
        match payload {
            Payload::Span(start, end) => &self.buf[start..end],
            Payload::Joined => &self.joined,
        }
    }
}

/// Reads fields out of one payload. Integers are little-endian throughout.
pub(super) struct Fields<'a> {
    bytes: &'a [u8],
    pub at: usize,
}

impl<'a> Fields<'a> {
    pub(super) fn new(bytes: &'a [u8], at: usize) -> Fields<'a> {
        Fields { bytes, at }
    }

    pub(super) fn done(&self) -> bool {
        self.at >= self.bytes.len()
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let slice = self
            .bytes
            .get(self.at..self.at + n)
            .ok_or_else(|| "a packet ended before its fields did".to_string())?;
        self.at += n;
        Ok(slice)
    }

    pub(super) fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    pub(super) fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }

    pub(super) fn u24(&mut self) -> Result<u32, String> {
        let b = self.take(3)?;
        Ok(b[0] as u32 | (b[1] as u32) << 8 | (b[2] as u32) << 16)
    }

    pub(super) fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    /// A length-encoded integer; `None` is `0xFB`, which in a text row is NULL.
    pub(super) fn lenenc(&mut self) -> Result<Option<u64>, String> {
        let first = self.u8()?;
        Ok(match first {
            0..=0xfa => Some(first as u64),
            0xfb => None,
            0xfc => Some(self.u16()? as u64),
            0xfd => Some(self.u24()? as u64),
            0xfe => Some(u64::from_le_bytes(self.take(8)?.try_into().unwrap())),
            _ => return Err(format!("0x{first:x} is not a length-encoded integer")),
        })
    }

    /// A length-encoded integer that must be there: a count, not a value.
    pub(super) fn count(&mut self) -> Result<u64, String> {
        Ok(self.lenenc()?.unwrap_or(0))
    }

    pub(super) fn bytes_of(&mut self, n: usize) -> Result<&'a [u8], String> {
        self.take(n)
    }

    pub(super) fn lenenc_string(&mut self) -> Result<String, String> {
        let n = self.count()? as usize;
        Ok(String::from_utf8_lossy(self.take(n)?).into_owned())
    }

    pub(super) fn skip_lenenc(&mut self) -> Result<(), String> {
        let n = self.count()? as usize;
        self.take(n).map(|_| ())
    }

    /// A NUL-terminated string (or the rest, if there is no NUL).
    pub(super) fn cstr(&mut self) -> Result<String, String> {
        let rest = self.bytes.get(self.at..).unwrap_or(&[]);
        let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
        let text = String::from_utf8_lossy(&rest[..end]).into_owned();
        self.at += (end + 1).min(rest.len());
        Ok(text)
    }

    pub(super) fn rest(&mut self) -> &'a [u8] {
        let rest = self.bytes.get(self.at..).unwrap_or(&[]);
        self.at = self.bytes.len();
        rest
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_small_payload_is_one_packet_with_its_sequence_id() {
        let mut out = Out::new(8);
        out.u8(0x03).bytes(b"SELECT 1");
        assert_eq!(
            out.finish(0),
            [
                9, 0, 0, 0, 0x03, b'S', b'E', b'L', b'E', b'C', b'T', b' ', b'1'
            ]
        );
    }

    #[test]
    fn a_payload_split_across_chunks_is_framed_once_whole() {
        let mut inbox = Inbox::default();
        inbox.push(&[3, 0, 0, 7, b'a']);
        assert!(inbox.next().is_none());
        inbox.push(b"bc");
        let payload = inbox.next().unwrap();
        assert_eq!(inbox.bytes(payload), b"abc");
        assert_eq!(inbox.seq, 7);
        assert_eq!(inbox.buffered(), 0);
    }

    #[test]
    fn a_continued_payload_is_joined_and_round_trips() {
        let mut out = Out::new(MAX_PAYLOAD + 10);
        out.bytes(&vec![7u8; MAX_PAYLOAD + 5]);
        let framed = out.finish(0);
        // Two packets: the full one, then the five bytes that continue it.
        assert_eq!(framed.len(), MAX_PAYLOAD + 5 + 8);
        let mut inbox = Inbox::default();
        inbox.push(&framed);
        let payload = inbox.next().unwrap();
        assert_eq!(inbox.bytes(payload).len(), MAX_PAYLOAD + 5);
        assert_eq!(inbox.seq, 1);
    }

    #[test]
    fn an_exact_multiple_of_the_limit_ends_with_an_empty_packet() {
        let mut out = Out::new(MAX_PAYLOAD);
        out.bytes(&vec![1u8; MAX_PAYLOAD]);
        let framed = out.finish(3);
        assert_eq!(framed.len(), MAX_PAYLOAD + 8);
        assert_eq!(&framed[MAX_PAYLOAD + 4..], [0, 0, 0, 4]);
    }

    #[test]
    fn length_encoded_integers_read_every_width_and_null() {
        let bytes = [0x05, 0xfb, 0xfc, 0x34, 0x12, 0xfd, 1, 2, 3];
        let mut fields = Fields::new(&bytes, 0);
        assert_eq!(fields.lenenc().unwrap(), Some(5));
        assert_eq!(fields.lenenc().unwrap(), None);
        assert_eq!(fields.lenenc().unwrap(), Some(0x1234));
        assert_eq!(fields.lenenc().unwrap(), Some(0x030201));
        assert!(fields.done());
        let mut out = Out::new(16);
        out.lenenc(0x1234).lenenc(0x030201);
        assert_eq!(&out.finish(0)[4..], [0xfc, 0x34, 0x12, 0xfd, 1, 2, 3]);
    }
}
