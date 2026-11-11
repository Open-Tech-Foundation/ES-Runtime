//! HTTP/2 frames (RFC 9113 §4, §6): reading one from a buffer, and writing
//! them.
//!
//! The reader checks everything a single frame can be checked for on its own
//! — its length against the size we allowed and its type's fixed size, the
//! stream it may or may not be on, its padding, its flags' fields, and the
//! values a SETTINGS frame may carry — and says, for every refusal, whether it
//! ends the stream or the whole connection (RFC 9113 §5.4). What depends on
//! the connection's state (which streams exist, the flow-control windows) is
//! the connection's to decide.

use super::{ErrorCode, H2Error};

/// The connection preface a client sends first (RFC 9113 §3.4).
pub const PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

/// Every frame starts with a 9-byte header.
pub const HEADER_LEN: usize = 9;

/// The protocol's default and smallest `SETTINGS_MAX_FRAME_SIZE`.
pub const DEFAULT_MAX_FRAME_SIZE: u32 = 16_384;

/// The largest `SETTINGS_MAX_FRAME_SIZE` may be (2^24 − 1).
pub const MAX_MAX_FRAME_SIZE: u32 = 16_777_215;

/// The largest a flow-control window may be (2^31 − 1).
pub const MAX_WINDOW: u32 = 0x7fff_ffff;

const DATA: u8 = 0x0;
const HEADERS: u8 = 0x1;
const PRIORITY: u8 = 0x2;
const RST_STREAM: u8 = 0x3;
const SETTINGS: u8 = 0x4;
const PUSH_PROMISE: u8 = 0x5;
const PING: u8 = 0x6;
const GOAWAY: u8 = 0x7;
const WINDOW_UPDATE: u8 = 0x8;
const CONTINUATION: u8 = 0x9;

const END_STREAM: u8 = 0x1;
const ACK: u8 = 0x1;
const END_HEADERS: u8 = 0x4;
const PADDED: u8 = 0x8;
const PRIORITY_FLAG: u8 = 0x20;

/// One setting from a SETTINGS frame (RFC 9113 §6.5.2); unknown identifiers
/// are skipped, as the RFC requires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    /// `SETTINGS_HEADER_TABLE_SIZE` (0x1).
    HeaderTableSize(u32),
    /// `SETTINGS_ENABLE_PUSH` (0x2): 0 or 1.
    EnablePush(bool),
    /// `SETTINGS_MAX_CONCURRENT_STREAMS` (0x3).
    MaxConcurrentStreams(u32),
    /// `SETTINGS_INITIAL_WINDOW_SIZE` (0x4): at most 2^31 − 1.
    InitialWindowSize(u32),
    /// `SETTINGS_MAX_FRAME_SIZE` (0x5): 2^14 to 2^24 − 1.
    MaxFrameSize(u32),
    /// `SETTINGS_MAX_HEADER_LIST_SIZE` (0x6).
    MaxHeaderListSize(u32),
    /// `SETTINGS_ENABLE_CONNECT_PROTOCOL` (0x8, RFC 8441): 0 or 1.
    EnableConnectProtocol(bool),
}

impl Setting {
    fn id_and_value(self) -> (u16, u32) {
        match self {
            Setting::HeaderTableSize(v) => (0x1, v),
            Setting::EnablePush(v) => (0x2, u32::from(v)),
            Setting::MaxConcurrentStreams(v) => (0x3, v),
            Setting::InitialWindowSize(v) => (0x4, v),
            Setting::MaxFrameSize(v) => (0x5, v),
            Setting::MaxHeaderListSize(v) => (0x6, v),
            Setting::EnableConnectProtocol(v) => (0x8, u32::from(v)),
        }
    }
}

/// A frame read from a buffer. Payload fields borrow from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame<'a> {
    /// DATA (§6.1).
    Data {
        /// The stream.
        stream: u32,
        /// The sender's last frame on this stream.
        end_stream: bool,
        /// The data, padding removed.
        data: &'a [u8],
        /// The whole payload, padding included: what the frame counts
        /// against the flow-control windows.
        flow_len: u32,
    },
    /// HEADERS (§6.2): the first part of a field block. Its priority fields,
    /// if any, are read and dropped (RFC 9113 deprecates them).
    Headers {
        /// The stream.
        stream: u32,
        /// The sender's last frame on this stream.
        end_stream: bool,
        /// The field block is complete; otherwise CONTINUATION follows.
        end_headers: bool,
        /// This frame's part of the field block.
        block: &'a [u8],
    },
    /// PRIORITY (§6.3): deprecated, read and ignored.
    Priority {
        /// The stream.
        stream: u32,
    },
    /// RST_STREAM (§6.4).
    RstStream {
        /// The stream.
        stream: u32,
        /// Why the stream ended.
        code: ErrorCode,
    },
    /// SETTINGS (§6.5), values already checked against their ranges.
    Settings {
        /// An acknowledgement of our SETTINGS, carrying none.
        ack: bool,
        /// The settings, in order; unknown identifiers are left out.
        settings: Vec<Setting>,
    },
    /// PUSH_PROMISE (§6.6).
    PushPromise {
        /// The stream the promise is made on.
        stream: u32,
        /// The stream it reserves.
        promised: u32,
        /// The field block is complete; otherwise CONTINUATION follows.
        end_headers: bool,
        /// This frame's part of the field block.
        block: &'a [u8],
    },
    /// PING (§6.7).
    Ping {
        /// A reply to our PING.
        ack: bool,
        /// The opaque 8 bytes, echoed back.
        data: [u8; 8],
    },
    /// GOAWAY (§6.8).
    GoAway {
        /// The highest stream the sender may have processed.
        last_stream: u32,
        /// Why the connection is ending.
        code: ErrorCode,
        /// Opaque diagnostic data.
        debug: &'a [u8],
    },
    /// WINDOW_UPDATE (§6.9).
    WindowUpdate {
        /// The stream, or 0 for the connection's window.
        stream: u32,
        /// How much the window grows: 1 to 2^31 − 1.
        increment: u32,
    },
    /// CONTINUATION (§6.10): more of a field block.
    Continuation {
        /// The stream.
        stream: u32,
        /// The field block is complete.
        end_headers: bool,
        /// This frame's part of the field block.
        block: &'a [u8],
    },
    /// A frame of a type this implementation does not know: to be ignored
    /// (§4.1), except that it may not interrupt a field block.
    Unknown {
        /// The frame type.
        kind: u8,
        /// The stream it was sent on.
        stream: u32,
    },
}

fn connection(code: ErrorCode, why: &'static str) -> H2Error {
    H2Error::Connection { code, why }
}

fn stream_error(stream: u32, code: ErrorCode, why: &'static str) -> H2Error {
    H2Error::Stream { stream, code, why }
}

/// Reads the frame at the start of `buf`: the frame and how many bytes it
/// took, or `None` when the whole frame has not arrived yet.
///
/// `max_frame_size` is our `SETTINGS_MAX_FRAME_SIZE`: a longer frame is a
/// `FRAME_SIZE_ERROR` (§4.2) — on the connection when the frame could change
/// connection state (a field block, SETTINGS, anything on stream 0), on its
/// stream otherwise.
///
/// # Errors
///
/// An [`H2Error`] naming the stream or connection the error ends.
pub fn parse_frame(buf: &[u8], max_frame_size: u32) -> Result<Option<(Frame<'_>, usize)>, H2Error> {
    let Some(header) = buf.get(..HEADER_LEN) else {
        return Ok(None);
    };
    let len = u32::from_be_bytes([0, header[0], header[1], header[2]]);
    let kind = header[3];
    let flags = header[4];
    let stream = u32::from_be_bytes([header[5], header[6], header[7], header[8]]) & 0x7fff_ffff;
    if len > max_frame_size {
        let alters_connection =
            stream == 0 || matches!(kind, HEADERS | PUSH_PROMISE | CONTINUATION | SETTINGS);
        return Err(if alters_connection {
            connection(
                ErrorCode::FrameSizeError,
                "frame larger than SETTINGS_MAX_FRAME_SIZE",
            )
        } else {
            stream_error(
                stream,
                ErrorCode::FrameSizeError,
                "frame larger than SETTINGS_MAX_FRAME_SIZE",
            )
        });
    }
    let total = HEADER_LEN + len as usize;
    let Some(payload) = buf.get(HEADER_LEN..total) else {
        return Ok(None);
    };
    let frame = match kind {
        DATA => {
            require_stream(stream, "DATA on stream 0")?;
            let data = unpad(payload, flags)?;
            Frame::Data {
                stream,
                end_stream: flags & END_STREAM != 0,
                data,
                flow_len: len,
            }
        }
        HEADERS => {
            require_stream(stream, "HEADERS on stream 0")?;
            let mut block = unpad(payload, flags)?;
            if flags & PRIORITY_FLAG != 0 {
                let Some(rest) = block.get(5..) else {
                    return Err(connection(
                        ErrorCode::FrameSizeError,
                        "HEADERS too short for its priority",
                    ));
                };
                let depends_on =
                    u32::from_be_bytes([block[0], block[1], block[2], block[3]]) & 0x7fff_ffff;
                if depends_on == stream {
                    return Err(stream_error(
                        stream,
                        ErrorCode::ProtocolError,
                        "a stream cannot depend on itself",
                    ));
                }
                block = rest;
            }
            Frame::Headers {
                stream,
                end_stream: flags & END_STREAM != 0,
                end_headers: flags & END_HEADERS != 0,
                block,
            }
        }
        PRIORITY => {
            require_stream(stream, "PRIORITY on stream 0")?;
            if len != 5 {
                return Err(stream_error(
                    stream,
                    ErrorCode::FrameSizeError,
                    "PRIORITY is 5 bytes",
                ));
            }
            let depends_on =
                u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]) & 0x7fff_ffff;
            if depends_on == stream {
                return Err(stream_error(
                    stream,
                    ErrorCode::ProtocolError,
                    "a stream cannot depend on itself",
                ));
            }
            Frame::Priority { stream }
        }
        RST_STREAM => {
            require_stream(stream, "RST_STREAM on stream 0")?;
            if len != 4 {
                return Err(connection(
                    ErrorCode::FrameSizeError,
                    "RST_STREAM is 4 bytes",
                ));
            }
            Frame::RstStream {
                stream,
                code: ErrorCode::from(u32::from_be_bytes([
                    payload[0], payload[1], payload[2], payload[3],
                ])),
            }
        }
        SETTINGS => {
            if stream != 0 {
                return Err(connection(ErrorCode::ProtocolError, "SETTINGS on a stream"));
            }
            let ack = flags & ACK != 0;
            if ack && len != 0 {
                return Err(connection(
                    ErrorCode::FrameSizeError,
                    "a SETTINGS ACK carries nothing",
                ));
            }
            if !len.is_multiple_of(6) {
                return Err(connection(
                    ErrorCode::FrameSizeError,
                    "SETTINGS is a multiple of 6 bytes",
                ));
            }
            let mut settings = Vec::with_capacity(payload.len() / 6);
            for p in payload.as_chunks::<6>().0 {
                let id = u16::from_be_bytes([p[0], p[1]]);
                let value = u32::from_be_bytes([p[2], p[3], p[4], p[5]]);
                if let Some(setting) = setting(id, value)? {
                    settings.push(setting);
                }
            }
            Frame::Settings { ack, settings }
        }
        PUSH_PROMISE => {
            require_stream(stream, "PUSH_PROMISE on stream 0")?;
            let rest = unpad(payload, flags)?;
            let Some(block) = rest.get(4..) else {
                return Err(connection(
                    ErrorCode::FrameSizeError,
                    "PUSH_PROMISE too short",
                ));
            };
            Frame::PushPromise {
                stream,
                promised: u32::from_be_bytes([rest[0], rest[1], rest[2], rest[3]]) & 0x7fff_ffff,
                end_headers: flags & END_HEADERS != 0,
                block,
            }
        }
        PING => {
            if stream != 0 {
                return Err(connection(ErrorCode::ProtocolError, "PING on a stream"));
            }
            let data: [u8; 8] = payload
                .try_into()
                .map_err(|_| connection(ErrorCode::FrameSizeError, "PING is 8 bytes"))?;
            Frame::Ping {
                ack: flags & ACK != 0,
                data,
            }
        }
        GOAWAY => {
            if stream != 0 {
                return Err(connection(ErrorCode::ProtocolError, "GOAWAY on a stream"));
            }
            if len < 8 {
                return Err(connection(
                    ErrorCode::FrameSizeError,
                    "GOAWAY is at least 8 bytes",
                ));
            }
            Frame::GoAway {
                last_stream: u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]])
                    & 0x7fff_ffff,
                code: ErrorCode::from(u32::from_be_bytes([
                    payload[4], payload[5], payload[6], payload[7],
                ])),
                debug: &payload[8..],
            }
        }
        WINDOW_UPDATE => {
            if len != 4 {
                return Err(connection(
                    ErrorCode::FrameSizeError,
                    "WINDOW_UPDATE is 4 bytes",
                ));
            }
            let increment =
                u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]) & 0x7fff_ffff;
            if increment == 0 {
                // §6.9: on the connection's window a connection error, on a
                // stream's a stream error.
                return Err(if stream == 0 {
                    connection(ErrorCode::ProtocolError, "WINDOW_UPDATE of 0")
                } else {
                    stream_error(stream, ErrorCode::ProtocolError, "WINDOW_UPDATE of 0")
                });
            }
            Frame::WindowUpdate { stream, increment }
        }
        CONTINUATION => {
            require_stream(stream, "CONTINUATION on stream 0")?;
            Frame::Continuation {
                stream,
                end_headers: flags & END_HEADERS != 0,
                block: payload,
            }
        }
        kind => Frame::Unknown { kind, stream },
    };
    Ok(Some((frame, total)))
}

fn require_stream(stream: u32, why: &'static str) -> Result<(), H2Error> {
    if stream == 0 {
        Err(connection(ErrorCode::ProtocolError, why))
    } else {
        Ok(())
    }
}

/// The payload without its padding (§6.1): the Pad Length byte must leave
/// room for itself and the padding, or the frame is a `PROTOCOL_ERROR`.
fn unpad(payload: &[u8], flags: u8) -> Result<&[u8], H2Error> {
    if flags & PADDED == 0 {
        return Ok(payload);
    }
    let Some((&pad, rest)) = payload.split_first() else {
        return Err(connection(
            ErrorCode::FrameSizeError,
            "padded frame without a Pad Length",
        ));
    };
    let pad = usize::from(pad);
    if pad >= payload.len() {
        return Err(connection(
            ErrorCode::ProtocolError,
            "padding as long as the frame",
        ));
    }
    Ok(&rest[..rest.len() - pad])
}

/// One setting, checked against its range (§6.5.2); `None` for an identifier
/// this implementation does not know.
fn setting(id: u16, value: u32) -> Result<Option<Setting>, H2Error> {
    Ok(Some(match id {
        0x1 => Setting::HeaderTableSize(value),
        0x2 => match value {
            0 | 1 => Setting::EnablePush(value == 1),
            _ => {
                return Err(connection(
                    ErrorCode::ProtocolError,
                    "SETTINGS_ENABLE_PUSH is 0 or 1",
                ));
            }
        },
        0x3 => Setting::MaxConcurrentStreams(value),
        0x4 => {
            if value > MAX_WINDOW {
                return Err(connection(
                    ErrorCode::FlowControlError,
                    "SETTINGS_INITIAL_WINDOW_SIZE above 2^31-1",
                ));
            }
            Setting::InitialWindowSize(value)
        }
        0x5 => {
            if !(DEFAULT_MAX_FRAME_SIZE..=MAX_MAX_FRAME_SIZE).contains(&value) {
                return Err(connection(
                    ErrorCode::ProtocolError,
                    "SETTINGS_MAX_FRAME_SIZE out of range",
                ));
            }
            Setting::MaxFrameSize(value)
        }
        0x6 => Setting::MaxHeaderListSize(value),
        0x8 => match value {
            0 | 1 => Setting::EnableConnectProtocol(value == 1),
            _ => {
                return Err(connection(
                    ErrorCode::ProtocolError,
                    "SETTINGS_ENABLE_CONNECT_PROTOCOL is 0 or 1",
                ));
            }
        },
        _ => return Ok(None),
    }))
}

// ---- writing ------------------------------------------------------------------

fn push_header(out: &mut Vec<u8>, len: usize, kind: u8, flags: u8, stream: u32) {
    debug_assert!(len <= MAX_MAX_FRAME_SIZE as usize);
    let len = (len as u32).to_be_bytes();
    out.extend_from_slice(&[len[1], len[2], len[3], kind, flags]);
    out.extend_from_slice(&(stream & 0x7fff_ffff).to_be_bytes());
}

/// Appends one DATA frame; `data` must fit the peer's maximum frame size.
pub fn encode_data(out: &mut Vec<u8>, stream: u32, data: &[u8], end_stream: bool) {
    push_header(
        out,
        data.len(),
        DATA,
        if end_stream { END_STREAM } else { 0 },
        stream,
    );
    out.extend_from_slice(data);
}

/// Appends a field block as HEADERS and, when it is longer than
/// `max_frame_size`, CONTINUATION frames.
pub fn encode_headers(
    out: &mut Vec<u8>,
    stream: u32,
    block: &[u8],
    end_stream: bool,
    max_frame_size: u32,
) {
    let max = max_frame_size as usize;
    let mut chunks = block.chunks(max.max(1));
    let first = chunks.next().unwrap_or_default();
    let last = block.len() <= max;
    let mut flags = if end_stream { END_STREAM } else { 0 };
    if last {
        flags |= END_HEADERS;
    }
    push_header(out, first.len(), HEADERS, flags, stream);
    out.extend_from_slice(first);
    let mut rest = chunks.peekable();
    while let Some(chunk) = rest.next() {
        let flags = if rest.peek().is_none() {
            END_HEADERS
        } else {
            0
        };
        push_header(out, chunk.len(), CONTINUATION, flags, stream);
        out.extend_from_slice(chunk);
    }
}

/// Appends a SETTINGS frame carrying `settings`.
pub fn encode_settings(out: &mut Vec<u8>, settings: &[Setting]) {
    push_header(out, settings.len() * 6, SETTINGS, 0, 0);
    for s in settings {
        let (id, value) = s.id_and_value();
        out.extend_from_slice(&id.to_be_bytes());
        out.extend_from_slice(&value.to_be_bytes());
    }
}

/// Appends a SETTINGS acknowledgement.
pub fn encode_settings_ack(out: &mut Vec<u8>) {
    push_header(out, 0, SETTINGS, ACK, 0);
}

/// Appends a PING, or its acknowledgement.
pub fn encode_ping(out: &mut Vec<u8>, ack: bool, data: [u8; 8]) {
    push_header(out, 8, PING, if ack { ACK } else { 0 }, 0);
    out.extend_from_slice(&data);
}

/// Appends a GOAWAY.
pub fn encode_goaway(out: &mut Vec<u8>, last_stream: u32, code: ErrorCode, debug: &[u8]) {
    push_header(out, 8 + debug.len(), GOAWAY, 0, 0);
    out.extend_from_slice(&(last_stream & 0x7fff_ffff).to_be_bytes());
    out.extend_from_slice(&u32::from(code).to_be_bytes());
    out.extend_from_slice(debug);
}

/// Appends a RST_STREAM.
pub fn encode_rst_stream(out: &mut Vec<u8>, stream: u32, code: ErrorCode) {
    push_header(out, 4, RST_STREAM, 0, stream);
    out.extend_from_slice(&u32::from(code).to_be_bytes());
}

/// Appends a WINDOW_UPDATE; `increment` must be 1 to 2^31 − 1.
pub fn encode_window_update(out: &mut Vec<u8>, stream: u32, increment: u32) {
    debug_assert!((1..=MAX_WINDOW).contains(&increment));
    push_header(out, 4, WINDOW_UPDATE, 0, stream);
    out.extend_from_slice(&increment.to_be_bytes());
}
