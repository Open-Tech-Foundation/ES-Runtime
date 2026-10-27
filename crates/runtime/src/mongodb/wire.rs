//! `OP_MSG`, the one message format MongoDB 3.6 and later speak, and the only
//! one this driver sends (DECISIONS D186: the minimum server is 7.0).
//!
//! A message is a 16-byte header — length, request id, the id it answers, the
//! opcode — then flag bits and sections. Section 0 is the command document;
//! section 1 is a document sequence, which carries the documents of an insert,
//! update or delete as they were encoded, so a batch is never re-encoded into
//! one large command.

/// `OP_MSG`.
const OP_MSG: i32 = 2013;
/// `OP_COMPRESSED`: only ever sent after compression is negotiated, which this
/// driver does not do yet.
const OP_COMPRESSED: i32 = 2012;
const HEADER: usize = 16;

/// The CRC-32C trailer is present.
const CHECKSUM_PRESENT: u32 = 1;
/// The sender will send another message without being asked (exhaust).
const MORE_TO_COME: u32 = 1 << 1;

/// A server's message size limit before `hello` reports its own.
pub(super) const DEFAULT_MAX_MESSAGE: usize = 48_000_000;

/// A document sequence: section 1, named for the command field it fills.
pub(crate) struct Sequence {
    pub name: String,
    pub documents: Vec<Vec<u8>>,
}

/// One request: `body` is the command document, already BSON.
pub(super) fn message(
    request_id: i32,
    body: &[u8],
    sequence: Option<&Sequence>,
) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(HEADER + 5 + body.len());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&request_id.to_le_bytes());
    out.extend_from_slice(&0i32.to_le_bytes());
    out.extend_from_slice(&OP_MSG.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.push(0);
    out.extend_from_slice(body);
    if let Some(sequence) = sequence {
        out.push(1);
        let start = out.len();
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(sequence.name.as_bytes());
        out.push(0);
        for document in &sequence.documents {
            out.extend_from_slice(document);
        }
        let size = i32::try_from(out.len() - start)
            .map_err(|_| "a document sequence is larger than 2 GiB")?;
        out[start..start + 4].copy_from_slice(&size.to_le_bytes());
    }
    let len = i32::try_from(out.len()).map_err(|_| "a message is larger than 2 GiB")?;
    out[..4].copy_from_slice(&len.to_le_bytes());
    Ok(out)
}

/// Bytes read from the socket and not yet taken as a whole message.
#[derive(Default)]
pub(super) struct Inbox {
    buf: Vec<u8>,
}

impl Inbox {
    pub(super) fn push(&mut self, chunk: &[u8]) {
        self.buf.extend_from_slice(chunk);
    }

    /// The next whole message, if it has arrived. A length outside what a
    /// header and `max` allow is an error: the stream cannot be trusted past it.
    pub(super) fn take(&mut self, max: usize) -> Result<Option<Vec<u8>>, String> {
        let Some(head) = self.buf.get(..4) else {
            return Ok(None);
        };
        let len = i32::from_le_bytes([head[0], head[1], head[2], head[3]]);
        let len = usize::try_from(len)
            .ok()
            .filter(|&len| len >= HEADER + 5 && len <= max.saturating_add(HEADER))
            .ok_or_else(|| format!("the server sent a message of {len} bytes"))?;
        if self.buf.len() < len {
            return Ok(None);
        }
        let rest = self.buf.split_off(len);
        Ok(Some(std::mem::replace(&mut self.buf, rest)))
    }
}

/// The command reply in a message answering `request_id`: its section-0
/// document.
pub(super) fn reply(message: &[u8], request_id: i32) -> Result<Vec<u8>, String> {
    let int = |at: usize| {
        i32::from_le_bytes([
            message[at],
            message[at + 1],
            message[at + 2],
            message[at + 3],
        ])
    };
    let response_to = int(8);
    let opcode = int(12);
    if response_to != request_id {
        return Err(format!(
            "the server answered request {response_to}, not {request_id}"
        ));
    }
    match opcode {
        OP_MSG => {}
        OP_COMPRESSED => {
            return Err("the server sent a compressed reply that was not asked for".into());
        }
        other => return Err(format!("the server sent opcode {other}, not OP_MSG")),
    }
    #[expect(clippy::cast_sign_loss, reason = "flag bits")]
    let flags = int(16) as u32;
    if flags & MORE_TO_COME != 0 {
        return Err("the server sent an exhaust reply that was not asked for".into());
    }
    let end = if flags & CHECKSUM_PRESENT != 0 {
        message.len() - 4
    } else {
        message.len()
    };
    let mut at = HEADER + 4;
    let mut body = None;
    while at < end {
        let kind = message[at];
        at += 1;
        let len = message
            .get(at..at + 4)
            .map(|b| i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .and_then(|n| usize::try_from(n).ok())
            .filter(|&n| n >= 5 && at + n <= end)
            .ok_or("a reply section is malformed")?;
        match kind {
            0 if body.is_none() => body = Some(message[at..at + len].to_vec()),
            0 => return Err("a reply has two command documents".into()),
            1 => {}
            other => return Err(format!("a reply has a section of kind {other}")),
        }
        at += len;
    }
    body.ok_or_else(|| "a reply has no command document".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_message_frames_its_body_and_sequence() {
        let body = [5, 0, 0, 0, 0];
        let sequence = Sequence {
            name: "documents".into(),
            documents: vec![vec![5, 0, 0, 0, 0], vec![5, 0, 0, 0, 0]],
        };
        let out = message(7, &body, Some(&sequence)).unwrap();
        assert_eq!(
            i32::from_le_bytes(out[..4].try_into().unwrap()) as usize,
            out.len()
        );
        assert_eq!(i32::from_le_bytes(out[4..8].try_into().unwrap()), 7);
        assert_eq!(i32::from_le_bytes(out[12..16].try_into().unwrap()), OP_MSG);
        assert_eq!(out[20], 0);
        assert_eq!(&out[21..26], &body);
        assert_eq!(out[26], 1);
        // size covers itself, the name and its NUL, and both documents
        assert_eq!(
            i32::from_le_bytes(out[27..31].try_into().unwrap()),
            4 + 10 + 10
        );
    }

    #[test]
    fn a_reply_is_read_back_and_checked() {
        let mut sent = message(9, &[5, 0, 0, 0, 0], None).unwrap();
        sent[8..12].copy_from_slice(&9i32.to_le_bytes());
        assert_eq!(reply(&sent, 9).unwrap(), vec![5, 0, 0, 0, 0]);
        assert!(reply(&sent, 10).is_err(), "answers another request");
        let mut exhaust = sent.clone();
        exhaust[16] = 2;
        assert!(reply(&exhaust, 9).is_err());
    }

    #[test]
    fn the_inbox_waits_for_a_whole_message_and_refuses_a_bad_length() {
        let mut inbox = Inbox::default();
        let sent = message(1, &[5, 0, 0, 0, 0], None).unwrap();
        inbox.push(&sent[..10]);
        assert_eq!(inbox.take(DEFAULT_MAX_MESSAGE).unwrap(), None);
        inbox.push(&sent[10..]);
        assert_eq!(inbox.take(DEFAULT_MAX_MESSAGE).unwrap(), Some(sent));
        let mut bad = Inbox::default();
        bad.push(&i32::MAX.to_le_bytes());
        assert!(bad.take(DEFAULT_MAX_MESSAGE).is_err());
    }
}
