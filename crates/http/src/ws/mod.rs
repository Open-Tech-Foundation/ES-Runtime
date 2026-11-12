//! WebSocket framing (RFC 6455 §5–§7), sans-I/O, for either role.
//!
//! [`Decoder`] turns bytes into whole messages and control frames, enforcing
//! the protocol as it goes; [`encode_frame`] and [`encode_close`] write them.
//! The opening handshake is HTTP/1.1's ([`crate::h1`]); [`check_upgrade`] is
//! the one rule for which requests may become a WebSocket, shared by every
//! server path.
//!
//! What a peer may not do, each refused with the close code RFC 6455 §7.4.1
//! names for it:
//! - **1002, protocol error**: a reserved bit set (no extension is
//!   negotiated), a reserved opcode, a fragmented or oversized (> 125 bytes)
//!   control frame, a continuation with nothing to continue or a new message
//!   inside a fragmented one, a frame masked the wrong way for its direction
//!   (§5.1: client frames are masked, server frames are not), a length not in
//!   its minimal encoding or with the top bit set, a close frame of one byte or
//!   with a code not allowed on the wire.
//! - **1007, invalid payload**: a text message, or a close reason, that is not
//!   UTF-8 — found as soon as the bytes so far cannot begin valid UTF-8, not
//!   only once the message ends (Autobahn's "fail fast" cases).
//! - **1009, too big**: a frame or a message past [`Limits`].

/// Which end of the connection decodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Receives masked frames; sends unmasked ones.
    Server,
    /// Receives unmasked frames; sends masked ones.
    Client,
}

/// The largest frame and message a decoder accepts.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// The largest payload of one frame.
    pub max_frame: usize,
    /// The largest message, all its fragments together.
    pub max_message: usize,
}

impl Default for Limits {
    /// 16 MiB a frame and 64 MiB a message — the bounds the runtime had under
    /// tungstenite, whose defaults these were.
    fn default() -> Self {
        Limits {
            max_frame: 16 << 20,
            max_message: 64 << 20,
        }
    }
}

/// A frame's opcode (§5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opcode {
    /// 0x0: the next fragment of a message.
    Continuation,
    /// 0x1: a UTF-8 text message.
    Text,
    /// 0x2: a binary message.
    Binary,
    /// 0x8: the closing handshake.
    Close,
    /// 0x9: a ping, to be answered with a pong.
    Ping,
    /// 0xA: a pong.
    Pong,
}

impl Opcode {
    fn from_bits(bits: u8) -> Option<Opcode> {
        Some(match bits {
            0x0 => Opcode::Continuation,
            0x1 => Opcode::Text,
            0x2 => Opcode::Binary,
            0x8 => Opcode::Close,
            0x9 => Opcode::Ping,
            0xA => Opcode::Pong,
            _ => return None,
        })
    }

    fn bits(self) -> u8 {
        match self {
            Opcode::Continuation => 0x0,
            Opcode::Text => 0x1,
            Opcode::Binary => 0x2,
            Opcode::Close => 0x8,
            Opcode::Ping => 0x9,
            Opcode::Pong => 0xA,
        }
    }

    fn is_control(self) -> bool {
        matches!(self, Opcode::Close | Opcode::Ping | Opcode::Pong)
    }
}

/// What a peer sent: a whole message or a control frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A text message, its fragments joined.
    Text(String),
    /// A binary message, its fragments joined.
    Binary(Vec<u8>),
    /// A ping: answer it with a pong carrying the same bytes.
    Ping(Vec<u8>),
    /// A pong.
    Pong(Vec<u8>),
    /// The peer began (or answered) the closing handshake. `code` is `None`
    /// when the frame carried none (§7.1.5: reported as 1005).
    Close {
        /// The status code, when one was sent.
        code: Option<u16>,
        /// The reason, UTF-8, possibly empty.
        reason: String,
    },
}

/// A protocol violation, and the close code to fail the connection with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WsError {
    /// The status code for the close frame sent in answer (§7.4.1).
    pub code: u16,
    /// What was wrong, for the log.
    pub why: &'static str,
}

impl std::fmt::Display for WsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "WebSocket error {}: {}", self.code, self.why)
    }
}

impl std::error::Error for WsError {}

/// 1002: the peer broke the protocol.
pub const PROTOCOL_ERROR: u16 = 1002;
/// 1007: a text message or close reason that is not UTF-8.
pub const INVALID_PAYLOAD: u16 = 1007;
/// 1009: a frame or message past the limits.
pub const TOO_BIG: u16 = 1009;

fn protocol(why: &'static str) -> WsError {
    WsError {
        code: PROTOCOL_ERROR,
        why,
    }
}

/// Whether `code` may appear in a close frame on the wire (§7.4): the codes
/// RFC 6455 defines for that, the ones IANA has registered since (1012–1014),
/// and the ranges for libraries (3000–3999) and applications (4000–4999).
/// 1005, 1006 and 1015 are for reporting only, never sent.
#[must_use]
pub fn is_sendable_close_code(code: u16) -> bool {
    matches!(code, 1000..=1003 | 1007..=1014 | 3000..=4999)
}

/// A message being assembled from fragments.
struct Partial {
    text: bool,
    data: Vec<u8>,
    /// How much of `data` is known to be valid UTF-8 (text only).
    valid: usize,
}

/// Turns bytes into messages and control frames.
pub struct Decoder {
    role: Role,
    limits: Limits,
    partial: Option<Partial>,
}

impl Decoder {
    /// A decoder for the `role` end.
    #[must_use]
    pub fn new(role: Role, limits: Limits) -> Self {
        Decoder {
            role,
            limits,
            partial: None,
        }
    }

    /// Reads the next frame from `buf`. `Ok(None)` when `buf` holds no whole
    /// frame yet; otherwise how many bytes were used, and the event the frame
    /// completed, if any (a fragment that does not end its message completes
    /// none).
    ///
    /// # Errors
    ///
    /// A protocol violation: fail the connection with its close code.
    pub fn decode(&mut self, buf: &[u8]) -> Result<Option<(Option<Event>, usize)>, WsError> {
        let [b0, b1, ..] = *buf else {
            return Ok(None);
        };
        let fin = b0 & 0x80 != 0;
        if b0 & 0x70 != 0 {
            return Err(protocol(
                "a reserved bit is set, with no extension negotiated",
            ));
        }
        let opcode = Opcode::from_bits(b0 & 0x0F).ok_or(protocol("a reserved opcode"))?;
        let masked = b1 & 0x80 != 0;
        match (self.role, masked) {
            (Role::Server, false) => return Err(protocol("a client frame is not masked")),
            (Role::Client, true) => return Err(protocol("a server frame is masked")),
            _ => {}
        }
        let mut at = 2;
        let len = match b1 & 0x7F {
            126 => {
                let Some(b) = buf.get(2..4) else {
                    return Ok(None);
                };
                at = 4;
                let n = u64::from(u16::from_be_bytes([b[0], b[1]]));
                if n < 126 {
                    return Err(protocol("a length not in its shortest form"));
                }
                n
            }
            127 => {
                let Some(b) = buf.get(2..10) else {
                    return Ok(None);
                };
                at = 10;
                let n = u64::from_be_bytes(b.try_into().expect("eight bytes"));
                if n >> 63 != 0 {
                    return Err(protocol("a length with its top bit set"));
                }
                if n <= 0xFFFF {
                    return Err(protocol("a length not in its shortest form"));
                }
                n
            }
            n => u64::from(n),
        };
        if opcode.is_control() {
            if !fin {
                return Err(protocol("a fragmented control frame"));
            }
            if len > 125 {
                return Err(protocol("a control frame longer than 125 bytes"));
            }
        }
        if len > self.limits.max_frame as u64 {
            return Err(WsError {
                code: TOO_BIG,
                why: "a frame larger than the limit",
            });
        }
        let len = len as usize;
        // Checked before the payload has arrived, so an oversized message is
        // refused at its header rather than after it has been buffered.
        if !opcode.is_control() {
            let so_far = match (&self.partial, opcode) {
                (Some(p), Opcode::Continuation) => p.data.len(),
                (None, Opcode::Continuation) => {
                    return Err(protocol("a continuation with no message to continue"));
                }
                (Some(_), _) => return Err(protocol("a new message inside a fragmented one")),
                (None, _) => 0,
            };
            if so_far + len > self.limits.max_message {
                return Err(WsError {
                    code: TOO_BIG,
                    why: "a message larger than the limit",
                });
            }
        }
        let mask = if masked {
            let Some(m) = buf.get(at..at + 4) else {
                return Ok(None);
            };
            at += 4;
            Some([m[0], m[1], m[2], m[3]])
        } else {
            None
        };
        let Some(payload) = buf.get(at..at + len) else {
            return Ok(None);
        };
        let mut data = payload.to_vec();
        if let Some(mask) = mask {
            apply_mask(&mut data, mask);
        }
        let used = at + len;
        let event = match opcode {
            Opcode::Ping => Some(Event::Ping(data)),
            Opcode::Pong => Some(Event::Pong(data)),
            Opcode::Close => Some(close_event(&data)?),
            Opcode::Text | Opcode::Binary => {
                let text = opcode == Opcode::Text;
                let mut partial = Partial {
                    text,
                    data,
                    valid: 0,
                };
                check_utf8(&mut partial, fin)?;
                if fin {
                    Some(finish(partial))
                } else {
                    self.partial = Some(partial);
                    None
                }
            }
            Opcode::Continuation => {
                let mut partial = self.partial.take().expect("checked above");
                partial.data.extend_from_slice(&data);
                check_utf8(&mut partial, fin)?;
                if fin {
                    Some(finish(partial))
                } else {
                    self.partial = Some(partial);
                    None
                }
            }
        };
        Ok(Some((event, used)))
    }
}

/// Validates what is new in a text message: fails as soon as the bytes so
/// far cannot be the start of UTF-8; at the end, also when a sequence is left
/// incomplete.
fn check_utf8(partial: &mut Partial, fin: bool) -> Result<(), WsError> {
    if !partial.text {
        return Ok(());
    }
    let invalid = WsError {
        code: INVALID_PAYLOAD,
        why: "a text message that is not UTF-8",
    };
    match std::str::from_utf8(&partial.data[partial.valid..]) {
        Ok(_) => partial.valid = partial.data.len(),
        Err(e) if e.error_len().is_some() => return Err(invalid),
        Err(e) => {
            partial.valid += e.valid_up_to();
            if fin {
                return Err(invalid);
            }
        }
    }
    Ok(())
}

fn finish(partial: Partial) -> Event {
    if partial.text {
        Event::Text(String::from_utf8(partial.data).expect("validated as it arrived"))
    } else {
        Event::Binary(partial.data)
    }
}

fn close_event(data: &[u8]) -> Result<Event, WsError> {
    match data {
        [] => Ok(Event::Close {
            code: None,
            reason: String::new(),
        }),
        [_] => Err(protocol("a close frame of one byte")),
        [hi, lo, reason @ ..] => {
            let code = u16::from_be_bytes([*hi, *lo]);
            if !is_sendable_close_code(code) {
                return Err(protocol("a close code not allowed on the wire"));
            }
            let reason = std::str::from_utf8(reason).map_err(|_| WsError {
                code: INVALID_PAYLOAD,
                why: "a close reason that is not UTF-8",
            })?;
            Ok(Event::Close {
                code: Some(code),
                reason: reason.to_owned(),
            })
        }
    }
}

/// XORs `data` with the masking key (§5.3), a word at a time.
pub fn apply_mask(data: &mut [u8], mask: [u8; 4]) {
    let word = u32::from_ne_bytes(mask);
    let (chunks, rest) = data.as_chunks_mut::<4>();
    for chunk in chunks {
        *chunk = (u32::from_ne_bytes(*chunk) ^ word).to_ne_bytes();
    }
    for (i, b) in rest.iter_mut().enumerate() {
        *b ^= mask[i];
    }
}

/// Appends one frame. A client passes a fresh, unpredictable `mask` for every
/// frame (§5.3); a server passes `None`.
pub fn encode_frame(
    out: &mut Vec<u8>,
    opcode: Opcode,
    fin: bool,
    payload: &[u8],
    mask: Option<[u8; 4]>,
) {
    out.push(if fin { 0x80 } else { 0 } | opcode.bits());
    let mask_bit = if mask.is_some() { 0x80 } else { 0 };
    let len = payload.len();
    if len < 126 {
        out.push(mask_bit | len as u8);
    } else if len <= 0xFFFF {
        out.push(mask_bit | 126);
        out.extend_from_slice(&(len as u16).to_be_bytes());
    } else {
        out.push(mask_bit | 127);
        out.extend_from_slice(&(len as u64).to_be_bytes());
    }
    match mask {
        None => out.extend_from_slice(payload),
        Some(mask) => {
            out.extend_from_slice(&mask);
            let start = out.len();
            out.extend_from_slice(payload);
            apply_mask(&mut out[start..], mask);
        }
    }
}

/// Appends a close frame: `code` (with `reason`, cut to the 123 bytes a
/// control frame leaves it, at a character boundary), or no payload at all.
pub fn encode_close(out: &mut Vec<u8>, code: Option<u16>, reason: &str, mask: Option<[u8; 4]>) {
    let mut payload = Vec::with_capacity(2 + reason.len().min(123));
    if let Some(code) = code {
        payload.extend_from_slice(&code.to_be_bytes());
        let mut end = reason.len().min(123);
        while !reason.is_char_boundary(end) {
            end -= 1;
        }
        payload.extend_from_slice(&reason.as_bytes()[..end]);
    }
    encode_frame(out, Opcode::Close, true, &payload, mask);
}

/// Why a request may not become a WebSocket (RFC 6455 §4.2.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpgradeRefusal {
    /// Not a WebSocket request at all: no `Upgrade: websocket` with
    /// `Connection: upgrade`, or not a `GET`. An HTTP server serves it as an
    /// ordinary request.
    NotWebSocket,
    /// A WebSocket request that is malformed: a missing or malformed
    /// `Sec-WebSocket-Key` (base64 of 16 bytes). Answered `400`.
    BadKey,
    /// A `Sec-WebSocket-Version` other than 13. Answered `426` with
    /// `Sec-WebSocket-Version: 13` (§4.4).
    Version,
}

/// Checks an opening handshake request (§4.2.1) given its method and a
/// lookup of its header fields (case-insensitive names, first value), and
/// returns its `Sec-WebSocket-Key`.
///
/// # Errors
///
/// Why the request cannot be upgraded.
pub fn check_upgrade<'a>(
    method: &[u8],
    field: impl Fn(&[u8]) -> Option<&'a [u8]>,
) -> Result<&'a [u8], UpgradeRefusal> {
    let has_token = |name: &[u8], token: &[u8]| {
        field(name).is_some_and(|v| {
            v.split(|&b| b == b',')
                .any(|t| crate::fields::trim_ows(t).eq_ignore_ascii_case(token))
        })
    };
    if method != b"GET"
        || !has_token(b"upgrade", b"websocket")
        || !has_token(b"connection", b"upgrade")
    {
        return Err(UpgradeRefusal::NotWebSocket);
    }
    if field(b"sec-websocket-version").map(crate::fields::trim_ows) != Some(b"13") {
        return Err(UpgradeRefusal::Version);
    }
    let key = field(b"sec-websocket-key")
        .map(crate::fields::trim_ows)
        .ok_or(UpgradeRefusal::BadKey)?;
    if !is_nonce(key) {
        return Err(UpgradeRefusal::BadKey);
    }
    Ok(key)
}

/// Whether `key` is base64 (with padding) of exactly 16 bytes: 22 characters
/// of the alphabet and `==`.
fn is_nonce(key: &[u8]) -> bool {
    let alphabet = |b: &u8| b.is_ascii_alphanumeric() || *b == b'+' || *b == b'/';
    // The 22nd character carries only 2 bits of the last byte: its low 4
    // bits must be zero for the encoding to be canonical.
    key.len() == 24
        && key[..22].iter().all(alphabet)
        && &key[22..] == b"=="
        && "AQgw".as_bytes().contains(&key[21])
}

#[cfg(test)]
mod tests;
