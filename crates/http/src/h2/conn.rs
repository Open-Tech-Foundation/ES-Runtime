//! An HTTP/2 connection (RFC 9113 §5, §6, §8), sans-I/O, for either role.
//!
//! The connection is fed what was read ([`Connection::receive`]), hands back
//! what happened ([`Connection::poll_event`]), takes what the application
//! sends ([`Connection::send_headers`], [`Connection::send_data`]), and keeps
//! what must be written ([`Connection::output`]). It owns no socket and no
//! timer; the driver owns both.
//!
//! Flow control runs both ways. What it receives counts against windows it
//! replenishes only as the application [`release`](Connection::release)s data
//! — a slow reader bounds what a peer can make it buffer. What it sends is
//! held to the peer's windows: [`send_data`](Connection::send_data) takes only
//! what fits and says how much, and [`Event::SendCapacity`] says when more
//! fits.
//!
//! The attacks HTTP/2 is known for are bounded here (DECISIONS D217):
//! - **Rapid Reset** (CVE-2023-44487): what the attack costs is work still
//!   running for requests already cancelled. Streams the peer resets while the
//!   application is still on them are counted until it says it is done
//!   ([`Connection::done_with`]); past [`Config::max_pending_resets`] at once
//!   the connection ends, `ENHANCE_YOUR_CALM`. Ordinary cancellations, which
//!   finish, never accumulate.
//! - **CONTINUATION flood**: one field block may carry at most
//!   [`Config::max_header_block`] compressed bytes and
//!   [`MAX_CONTINUATIONS`] CONTINUATION frames.
//! - **HPACK bomb**: the decoded list is held to
//!   [`Config::max_header_list_size`]; a request past it is answered `431`.
//! - **SETTINGS, PING and empty-frame floods**: replies we owe are capped
//!   while unsent ([`MAX_PENDING_OUTPUT`]), and so is a run of empty frames.

use std::collections::{HashMap, HashSet, VecDeque};

use super::frame::{
    DEFAULT_MAX_FRAME_SIZE, Frame, MAX_WINDOW, PREFACE, Setting, encode_data, encode_goaway,
    encode_headers, encode_ping, encode_rst_stream, encode_settings, encode_settings_ack,
    encode_window_update, parse_frame,
};
use super::headers::{Pseudo, Section, check, content_length};
use super::{ErrorCode, H2Error};
use crate::hpack::{Decoder, Encoder};

/// The window every stream and the connection start with (RFC 9113 §6.9.2).
const DEFAULT_WINDOW: i64 = 65_535;

/// The most CONTINUATION frames one field block may take.
pub const MAX_CONTINUATIONS: usize = 64;

/// The most bytes of replies we owe (SETTINGS ACKs, PING ACKs, resets) that
/// may wait unsent: a peer that asks faster than it reads the answers is
/// flooding, not talking.
pub const MAX_PENDING_OUTPUT: usize = 1 << 20;

/// The longest run of empty DATA frames (no data, no END_STREAM) accepted.
const MAX_EMPTY_FRAMES: u32 = 100;

/// Which end of the connection this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// Accepts streams the peer opens (odd ids).
    Server,
    /// Opens streams (odd ids) and reads their responses.
    Client,
}

/// What a connection allows its peer.
#[derive(Debug, Clone, Copy)]
pub struct Config {
    /// `SETTINGS_MAX_CONCURRENT_STREAMS` (D42: 256).
    pub max_concurrent_streams: u32,
    /// `SETTINGS_INITIAL_WINDOW_SIZE`: each stream's receive window.
    pub stream_window: u32,
    /// The connection's receive window, raised from 65,535 at the start.
    pub connection_window: u32,
    /// `SETTINGS_MAX_HEADER_LIST_SIZE`: the decoded list a field block may
    /// make.
    pub max_header_list_size: u32,
    /// The compressed bytes one field block may take, CONTINUATIONs included.
    pub max_header_block: usize,
    /// Streams the peer may reset before the application is done with them.
    pub max_pending_resets: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            max_concurrent_streams: 256,
            stream_window: 1 << 20,
            connection_window: 2 << 20,
            max_header_list_size: 16 * 1024,
            max_header_block: 64 * 1024,
            max_pending_resets: 100,
        }
    }
}

/// Something that happened on the connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A stream's opening field section: a request (server) or a response
    /// (client; `1xx` interim responses included).
    Headers {
        /// The stream.
        stream: u32,
        /// Its pseudo-header fields.
        pseudo: Pseudo,
        /// Its regular fields, in order.
        fields: Vec<(Vec<u8>, Vec<u8>)>,
        /// No body follows.
        end_stream: bool,
    },
    /// Body bytes. Release them with [`Connection::release`] once consumed.
    Data {
        /// The stream.
        stream: u32,
        /// The bytes.
        data: Vec<u8>,
        /// The body is complete.
        end_stream: bool,
    },
    /// The trailer section, which ends the body.
    Trailers {
        /// The stream.
        stream: u32,
        /// The trailer fields.
        fields: Vec<(Vec<u8>, Vec<u8>)>,
    },
    /// The peer reset the stream, or the connection reset it for a stream
    /// error; nothing more will come on it, and nothing more may be sent.
    Reset {
        /// The stream.
        stream: u32,
        /// Why.
        code: ErrorCode,
    },
    /// More of `stream` (or, for 0, of the connection) may be sent now.
    SendCapacity {
        /// The stream, or 0.
        stream: u32,
    },
    /// The peer is going away: streams above `last_stream` that we opened
    /// were not processed and may be retried elsewhere.
    GoAway {
        /// The highest stream the peer may have processed.
        last_stream: u32,
        /// Why.
        code: ErrorCode,
    },
}

#[derive(Debug)]
struct Stream {
    /// The peer sent END_STREAM (or the stream was reset).
    recv_closed: bool,
    /// We sent END_STREAM.
    send_closed: bool,
    /// The opening field section has arrived.
    headers_received: bool,
    send_window: i64,
    recv_window: i64,
    /// Bytes the application released, not yet given back to the peer.
    recv_released: u32,
    content_length: Option<u64>,
    data_received: u64,
    /// The application has not finished with it: it counts toward the reset
    /// budget if the peer resets it now.
    pending: bool,
    /// A `1xx` arrived (client): the final response is still to come.
    interim: bool,
    /// The request was `HEAD` (client): its response has no content,
    /// whatever `content-length` it states (RFC 9110 §9.3.2).
    head: bool,
}

/// A field block being assembled across CONTINUATION frames.
struct Partial {
    stream: u32,
    end_stream: bool,
    block: Vec<u8>,
    continuations: usize,
}

/// One HTTP/2 connection's state.
pub struct Connection {
    role: Role,
    config: Config,
    /// The client's preface still to be read (server) or nothing (client).
    preface_left: usize,
    streams: HashMap<u32, Stream>,
    /// The highest stream the peer opened, and the next one we will.
    last_peer_stream: u32,
    next_local_stream: u32,
    decoder: Decoder,
    encoder: Encoder,
    partial: Option<Partial>,
    events: VecDeque<Event>,
    out: Vec<u8>,
    // The peer's settings.
    peer_max_frame_size: u32,
    peer_initial_window: i64,
    peer_max_streams: u32,
    /// The peer's first SETTINGS has arrived: its limits are known.
    settings_received: bool,
    // Flow control on the connection itself.
    send_window: i64,
    recv_window: i64,
    recv_released: u32,
    /// Streams the peer reset while the application was still on them.
    pending_resets: HashSet<u32>,
    empty_frames: u32,
    /// We sent GOAWAY: new streams above this are refused.
    goaway_sent: Option<u32>,
    /// The peer sent GOAWAY.
    goaway_received: Option<u32>,
    /// A connection error ended it; nothing more is read.
    failed: Option<H2Error>,
}

impl Connection {
    /// A server connection: our SETTINGS are queued, and the client's preface
    /// is expected first.
    #[must_use]
    pub fn server(config: Config) -> Self {
        Self::new(Role::Server, config)
    }

    /// A client connection: the preface and our SETTINGS are queued.
    #[must_use]
    pub fn client(config: Config) -> Self {
        Self::new(Role::Client, config)
    }

    fn new(role: Role, config: Config) -> Self {
        let mut c = Connection {
            role,
            config,
            preface_left: if role == Role::Server {
                PREFACE.len()
            } else {
                0
            },
            streams: HashMap::new(),
            last_peer_stream: 0,
            next_local_stream: 1,
            decoder: Decoder::new(4096),
            encoder: Encoder::new(4096),
            partial: None,
            events: VecDeque::new(),
            out: Vec::with_capacity(4096),
            peer_max_frame_size: DEFAULT_MAX_FRAME_SIZE,
            peer_initial_window: DEFAULT_WINDOW,
            peer_max_streams: u32::MAX,
            settings_received: false,
            send_window: DEFAULT_WINDOW,
            recv_window: DEFAULT_WINDOW,
            recv_released: 0,
            pending_resets: HashSet::new(),
            empty_frames: 0,
            goaway_sent: None,
            goaway_received: None,
            failed: None,
        };
        if role == Role::Client {
            c.out.extend_from_slice(PREFACE);
        }
        let mut settings = vec![
            Setting::MaxConcurrentStreams(config.max_concurrent_streams),
            Setting::InitialWindowSize(config.stream_window),
            Setting::MaxHeaderListSize(config.max_header_list_size),
        ];
        if role == Role::Client {
            settings.push(Setting::EnablePush(false));
        }
        encode_settings(&mut c.out, &settings);
        let raise = i64::from(config.connection_window) - DEFAULT_WINDOW;
        if raise > 0 {
            encode_window_update(&mut c.out, 0, raise as u32);
            c.recv_window += raise;
        }
        c
    }

    // ---- input --------------------------------------------------------------

    /// Reads frames from `input`; returns how many bytes it used (the rest is
    /// an incomplete frame, to be passed again with more). Events queue up for
    /// [`poll_event`](Self::poll_event); replies queue up in the output.
    ///
    /// # Errors
    ///
    /// A connection error. GOAWAY is already queued; write the output, then
    /// close.
    pub fn receive(&mut self, input: &[u8]) -> Result<usize, H2Error> {
        if let Some(e) = self.failed {
            return Err(e);
        }
        let mut at = 0;
        if self.preface_left > 0 {
            let want = &PREFACE[PREFACE.len() - self.preface_left..];
            let n = want.len().min(input.len());
            if input[..n] != want[..n] {
                return Err(self.fail(ErrorCode::ProtocolError, "no HTTP/2 connection preface"));
            }
            self.preface_left -= n;
            at = n;
            if self.preface_left > 0 {
                return Ok(at);
            }
        }
        loop {
            match parse_frame(&input[at..], DEFAULT_MAX_FRAME_SIZE) {
                Ok(None) => return Ok(at),
                Ok(Some((frame, used))) => {
                    at += used;
                    if let Err(e) = self.on_frame(frame) {
                        self.on_error(e)?;
                    }
                }
                Err(e) => {
                    let len = u32::from_be_bytes([0, input[at], input[at + 1], input[at + 2]]);
                    if len > DEFAULT_MAX_FRAME_SIZE {
                        // Too large to skip safely before it has all arrived:
                        // ended here, as RFC 9113 §4.2 permits for any frame.
                        return Err(self.fail(
                            ErrorCode::FrameSizeError,
                            "frame larger than SETTINGS_MAX_FRAME_SIZE",
                        ));
                    }
                    // Any other error a frame carries on its own is found once
                    // it has arrived whole: that frame is skipped, exactly.
                    at += 9 + len as usize;
                    self.on_error(e)?;
                }
            }
            if self.out.len() > MAX_PENDING_OUTPUT {
                return Err(self.fail(ErrorCode::EnhanceYourCalm, "replies owed faster than read"));
            }
        }
    }

    /// Handles an error from one frame: a stream error resets the stream and
    /// carries on; a connection error ends everything.
    fn on_error(&mut self, e: H2Error) -> Result<(), H2Error> {
        match e {
            H2Error::Stream { stream, code, .. } => {
                self.reset_stream(stream, code);
                Ok(())
            }
            H2Error::Connection { code, why } => Err(self.fail(code, why)),
        }
    }

    fn fail(&mut self, code: ErrorCode, why: &'static str) -> H2Error {
        let e = H2Error::Connection { code, why };
        if self.failed.is_none() {
            encode_goaway(&mut self.out, self.last_peer_stream, code, why.as_bytes());
            self.failed = Some(e);
        }
        e
    }

    fn connection(code: ErrorCode, why: &'static str) -> H2Error {
        H2Error::Connection { code, why }
    }

    fn on_frame(&mut self, frame: Frame<'_>) -> Result<(), H2Error> {
        // Mid-block, only the block's own CONTINUATION may come (§6.10).
        if let Some(partial) = &self.partial
            && !matches!(frame, Frame::Continuation { stream, .. } if stream == partial.stream)
        {
            return Err(Self::connection(
                ErrorCode::ProtocolError,
                "a frame interrupted a field block",
            ));
        }
        match frame {
            Frame::Settings { ack, settings } => self.on_settings(ack, &settings),
            Frame::Ping { ack, data } => {
                if !ack {
                    encode_ping(&mut self.out, true, data);
                }
                Ok(())
            }
            Frame::GoAway {
                last_stream, code, ..
            } => {
                self.goaway_received = Some(last_stream);
                self.events.push_back(Event::GoAway { last_stream, code });
                Ok(())
            }
            Frame::WindowUpdate { stream, increment } => self.on_window_update(stream, increment),
            Frame::Headers {
                stream,
                end_stream,
                end_headers,
                block,
            } => {
                if end_headers {
                    self.on_field_block(stream, end_stream, block.to_vec())
                } else {
                    if block.len() > self.config.max_header_block {
                        return Err(Self::connection(
                            ErrorCode::EnhanceYourCalm,
                            "field block too large",
                        ));
                    }
                    self.partial = Some(Partial {
                        stream,
                        end_stream,
                        block: block.to_vec(),
                        continuations: 0,
                    });
                    Ok(())
                }
            }
            Frame::Continuation {
                stream,
                end_headers,
                block,
            } => {
                let Some(mut partial) = self.partial.take() else {
                    return Err(Self::connection(
                        ErrorCode::ProtocolError,
                        "CONTINUATION without a field block",
                    ));
                };
                debug_assert_eq!(partial.stream, stream);
                partial.continuations += 1;
                partial.block.extend_from_slice(block);
                if partial.continuations > MAX_CONTINUATIONS
                    || partial.block.len() > self.config.max_header_block
                {
                    return Err(Self::connection(
                        ErrorCode::EnhanceYourCalm,
                        "field block too large",
                    ));
                }
                if end_headers {
                    self.on_field_block(partial.stream, partial.end_stream, partial.block)
                } else {
                    self.partial = Some(partial);
                    Ok(())
                }
            }
            Frame::Data {
                stream,
                end_stream,
                data,
                flow_len,
            } => self.on_data(stream, end_stream, data, flow_len),
            Frame::RstStream { stream, code } => {
                if self.is_idle(stream) {
                    return Err(Self::connection(
                        ErrorCode::ProtocolError,
                        "RST_STREAM on an idle stream",
                    ));
                }
                if let Some(s) = self.streams.remove(&stream) {
                    if s.pending {
                        self.pending_resets.insert(stream);
                        if self.pending_resets.len() > self.config.max_pending_resets {
                            return Err(Self::connection(
                                ErrorCode::EnhanceYourCalm,
                                "too many streams reset",
                            ));
                        }
                    }
                    self.events.push_back(Event::Reset { stream, code });
                }
                Ok(())
            }
            Frame::PushPromise { .. } => {
                // We never enable push: as a server a client cannot push, and
                // as a client we said SETTINGS_ENABLE_PUSH = 0.
                Err(Self::connection(
                    ErrorCode::ProtocolError,
                    "PUSH_PROMISE was not enabled",
                ))
            }
            Frame::Priority { .. } | Frame::Unknown { .. } => Ok(()),
        }
    }

    fn on_settings(&mut self, ack: bool, settings: &[Setting]) -> Result<(), H2Error> {
        if ack {
            // Our settings are in force now; ours never shrink the table.
            return Ok(());
        }
        for setting in settings {
            match *setting {
                Setting::HeaderTableSize(size) => {
                    self.encoder.set_max_table_size((size as usize).min(4096));
                }
                Setting::InitialWindowSize(size) => {
                    let delta = i64::from(size) - self.peer_initial_window;
                    self.peer_initial_window = i64::from(size);
                    for (&id, s) in &mut self.streams {
                        s.send_window += delta;
                        if s.send_window > i64::from(MAX_WINDOW) {
                            return Err(Self::connection(
                                ErrorCode::FlowControlError,
                                "a stream window above 2^31-1",
                            ));
                        }
                        if delta > 0 && s.send_window > 0 && !s.send_closed {
                            self.events.push_back(Event::SendCapacity { stream: id });
                        }
                    }
                }
                Setting::MaxFrameSize(size) => self.peer_max_frame_size = size,
                Setting::MaxConcurrentStreams(n) => self.peer_max_streams = n,
                Setting::EnablePush(true) if self.role == Role::Client => {
                    // §6.5.2: a server must not send ENABLE_PUSH = 1.
                    return Err(Self::connection(
                        ErrorCode::ProtocolError,
                        "a server enabled push",
                    ));
                }
                Setting::EnablePush(_)
                | Setting::MaxHeaderListSize(_)
                | Setting::EnableConnectProtocol(_) => {}
            }
        }
        self.settings_received = true;
        encode_settings_ack(&mut self.out);
        Ok(())
    }

    fn on_window_update(&mut self, stream: u32, increment: u32) -> Result<(), H2Error> {
        if stream == 0 {
            self.send_window += i64::from(increment);
            if self.send_window > i64::from(MAX_WINDOW) {
                return Err(Self::connection(
                    ErrorCode::FlowControlError,
                    "the connection window above 2^31-1",
                ));
            }
            self.events.push_back(Event::SendCapacity { stream: 0 });
            return Ok(());
        }
        if self.is_idle(stream) {
            return Err(Self::connection(
                ErrorCode::ProtocolError,
                "WINDOW_UPDATE on an idle stream",
            ));
        }
        let Some(s) = self.streams.get_mut(&stream) else {
            return Ok(()); // closed: a late update, harmless
        };
        s.send_window += i64::from(increment);
        if s.send_window > i64::from(MAX_WINDOW) {
            return Err(H2Error::Stream {
                stream,
                code: ErrorCode::FlowControlError,
                why: "a stream window above 2^31-1",
            });
        }
        if !s.send_closed {
            self.events.push_back(Event::SendCapacity { stream });
        }
        Ok(())
    }

    /// Whether `stream` has never been used: above the highest the peer (or,
    /// for our own ids, we) opened.
    fn is_idle(&self, stream: u32) -> bool {
        let peer_opens = match self.role {
            Role::Server => stream % 2 == 1,
            Role::Client => stream.is_multiple_of(2),
        };
        if peer_opens {
            stream > self.last_peer_stream
        } else {
            stream >= self.next_local_stream
        }
    }

    fn on_field_block(
        &mut self,
        stream: u32,
        end_stream: bool,
        block: Vec<u8>,
    ) -> Result<(), H2Error> {
        // Decoded whatever happens next: the table must stay in step.
        let mut list = Vec::new();
        let decoded = self
            .decoder
            .decode(&block, self.config.max_header_list_size as usize, &mut list)
            .map_err(|_| Self::connection(ErrorCode::CompressionError, "HPACK"))?;

        let existing = self.streams.contains_key(&stream);
        if !existing {
            return self.open_peer_stream(stream, end_stream, list, decoded.too_large);
        }
        let s = self.streams.get_mut(&stream).expect("checked");
        if s.recv_closed {
            return Err(H2Error::Stream {
                stream,
                code: ErrorCode::StreamClosed,
                why: "HEADERS after the peer ended the stream",
            });
        }
        if decoded.too_large {
            return Err(H2Error::Stream {
                stream,
                code: ErrorCode::ProtocolError,
                why: "field section too large",
            });
        }
        if !s.headers_received || s.interim {
            // The client's response (after any 1xx).
            return self.on_response_head(stream, end_stream, list);
        }
        // Trailers: they end the body, and carry no pseudo-header (§8.1).
        if !end_stream {
            return Err(H2Error::Stream {
                stream,
                code: ErrorCode::ProtocolError,
                why: "trailers without END_STREAM",
            });
        }
        let (_, fields) = check(stream, Section::Trailers, list)?;
        self.end_body(stream)?;
        self.events.push_back(Event::Trailers { stream, fields });
        Ok(())
    }

    fn open_peer_stream(
        &mut self,
        stream: u32,
        end_stream: bool,
        list: Vec<(Vec<u8>, Vec<u8>)>,
        too_large: bool,
    ) -> Result<(), H2Error> {
        if self.role == Role::Client {
            // A server opens no streams without push, which we disabled.
            return Err(Self::connection(
                ErrorCode::ProtocolError,
                "HEADERS on a stream we did not open",
            ));
        }
        if stream.is_multiple_of(2) {
            return Err(Self::connection(
                ErrorCode::ProtocolError,
                "a client opened an even stream",
            ));
        }
        if stream <= self.last_peer_stream {
            // Closed already (or skipped, which closes it): §5.1.1.
            return Err(Self::connection(
                ErrorCode::StreamClosed,
                "HEADERS on a closed stream",
            ));
        }
        self.last_peer_stream = stream;
        if self.goaway_sent.is_some_and(|last| stream > last) {
            // Going away: refused, so the client knows it may retry it.
            return Err(H2Error::Stream {
                stream,
                code: ErrorCode::RefusedStream,
                why: "the connection is going away",
            });
        }
        let open = self
            .streams
            .values()
            .filter(|s| !(s.recv_closed && s.send_closed))
            .count();
        if open >= self.config.max_concurrent_streams as usize {
            return Err(H2Error::Stream {
                stream,
                code: ErrorCode::RefusedStream,
                why: "SETTINGS_MAX_CONCURRENT_STREAMS reached",
            });
        }
        let mut s = Stream {
            recv_closed: end_stream,
            send_closed: false,
            headers_received: true,
            send_window: self.peer_initial_window,
            recv_window: i64::from(self.config.stream_window),
            recv_released: 0,
            content_length: None,
            data_received: 0,
            pending: true,
            interim: false,
            head: false,
        };
        if too_large {
            // §8.2.3 suggests 431; the stream stays ours to answer.
            self.streams.insert(stream, s);
            let mut block = Vec::new();
            self.encoder
                .encode([(&b":status"[..], &b"431"[..])], &mut block);
            encode_headers(
                &mut self.out,
                stream,
                &block,
                true,
                self.peer_max_frame_size,
            );
            if let Some(s) = self.streams.get_mut(&stream) {
                s.send_closed = true;
                s.pending = false;
            }
            if !end_stream {
                self.reset_stream(stream, ErrorCode::NoError);
            } else {
                self.streams.remove(&stream);
            }
            return Ok(());
        }
        let (pseudo, fields) = check(stream, Section::Request, list)?;
        s.content_length = content_length(stream, &fields)?;
        if end_stream && s.content_length.is_some_and(|n| n != 0) {
            return Err(H2Error::Stream {
                stream,
                code: ErrorCode::ProtocolError,
                why: "content-length without a body",
            });
        }
        self.streams.insert(stream, s);
        self.events.push_back(Event::Headers {
            stream,
            pseudo,
            fields,
            end_stream,
        });
        Ok(())
    }

    fn on_response_head(
        &mut self,
        stream: u32,
        end_stream: bool,
        list: Vec<(Vec<u8>, Vec<u8>)>,
    ) -> Result<(), H2Error> {
        let (pseudo, fields) = check(stream, Section::Response, list)?;
        let status = pseudo.status.unwrap_or(0);
        let interim = (100..200).contains(&status);
        if interim && end_stream {
            return Err(H2Error::Stream {
                stream,
                code: ErrorCode::ProtocolError,
                why: "a 1xx response with END_STREAM",
            });
        }
        let length = content_length(stream, &fields)?;
        let s = self.streams.get_mut(&stream).expect("checked");
        // A response with no content may still state the length a GET or a
        // 200 would have had (RFC 9110 §8.6): it frames nothing here.
        let length = if s.head || status == 204 || status == 304 {
            None
        } else {
            length
        };
        s.headers_received = true;
        s.interim = interim;
        if !interim {
            s.content_length = length;
            if end_stream {
                s.recv_closed = true;
            }
        }
        self.events.push_back(Event::Headers {
            stream,
            pseudo,
            fields,
            end_stream,
        });
        if end_stream {
            self.maybe_forget(stream);
        }
        Ok(())
    }

    fn on_data(
        &mut self,
        stream: u32,
        end_stream: bool,
        data: &[u8],
        flow_len: u32,
    ) -> Result<(), H2Error> {
        // Counted against the connection whatever the stream's state (§6.9).
        self.recv_window -= i64::from(flow_len);
        if self.recv_window < 0 {
            return Err(Self::connection(
                ErrorCode::FlowControlError,
                "the connection window was overrun",
            ));
        }
        if self.is_idle(stream) {
            return Err(Self::connection(
                ErrorCode::ProtocolError,
                "DATA on an idle stream",
            ));
        }
        if data.is_empty() && !end_stream {
            self.empty_frames += 1;
            if self.empty_frames > MAX_EMPTY_FRAMES {
                return Err(Self::connection(
                    ErrorCode::EnhanceYourCalm,
                    "a run of empty frames",
                ));
            }
        } else {
            self.empty_frames = 0;
        }
        let Some(s) = self.streams.get_mut(&stream) else {
            // Closed: give the connection credit back, and say so.
            self.release_connection(flow_len);
            return Err(H2Error::Stream {
                stream,
                code: ErrorCode::StreamClosed,
                why: "DATA on a closed stream",
            });
        };
        if s.recv_closed || !s.headers_received || s.interim {
            let code = if s.recv_closed {
                ErrorCode::StreamClosed
            } else {
                ErrorCode::ProtocolError
            };
            self.release_connection(flow_len);
            return Err(H2Error::Stream {
                stream,
                code,
                why: "DATA where none may come",
            });
        }
        s.recv_window -= i64::from(flow_len);
        if s.recv_window < 0 {
            return Err(H2Error::Stream {
                stream,
                code: ErrorCode::FlowControlError,
                why: "the stream window was overrun",
            });
        }
        s.data_received += data.len() as u64;
        if s.content_length.is_some_and(|n| s.data_received > n) {
            return Err(H2Error::Stream {
                stream,
                code: ErrorCode::ProtocolError,
                why: "more data than content-length",
            });
        }
        // Padding is credit the application never sees: given back now.
        let padding = flow_len - data.len() as u32;
        if padding > 0 {
            self.release(stream, padding as usize);
        }
        if end_stream {
            self.end_body(stream)?;
        }
        self.events.push_back(Event::Data {
            stream,
            data: data.to_vec(),
            end_stream,
        });
        Ok(())
    }

    /// The body ended: its length must match a stated `content-length`.
    fn end_body(&mut self, stream: u32) -> Result<(), H2Error> {
        let s = self.streams.get_mut(&stream).expect("an open stream");
        s.recv_closed = true;
        if s.content_length.is_some_and(|n| n != s.data_received) {
            return Err(H2Error::Stream {
                stream,
                code: ErrorCode::ProtocolError,
                why: "data shorter than content-length",
            });
        }
        Ok(())
    }

    /// Resets `stream` from our side: RST_STREAM out, an event in.
    fn reset_stream(&mut self, stream: u32, code: ErrorCode) {
        encode_rst_stream(&mut self.out, stream, code);
        if self.streams.remove(&stream).is_some() {
            self.events.push_back(Event::Reset { stream, code });
        }
        if self.role == Role::Server && stream % 2 == 1 && stream > self.last_peer_stream {
            self.last_peer_stream = stream;
        }
    }

    fn maybe_forget(&mut self, stream: u32) {
        if self
            .streams
            .get(&stream)
            .is_some_and(|s| s.recv_closed && s.send_closed)
        {
            self.streams.remove(&stream);
        }
    }

    // ---- events and output ---------------------------------------------------

    /// The next thing that happened, if any.
    pub fn poll_event(&mut self) -> Option<Event> {
        self.events.pop_front()
    }

    /// The bytes waiting to be written.
    #[must_use]
    pub fn output(&self) -> &[u8] {
        &self.out
    }

    /// Marks `n` bytes of [`output`](Self::output) as written.
    pub fn consume_output(&mut self, n: usize) {
        self.out.drain(..n);
    }

    /// Whether a connection error ended the connection.
    #[must_use]
    pub fn is_failed(&self) -> bool {
        self.failed.is_some()
    }

    /// Whether no stream is open: after a GOAWAY either way, the connection
    /// may close once this holds and the output is written.
    #[must_use]
    pub fn is_idle_after_goaway(&self) -> bool {
        (self.goaway_sent.is_some() || self.goaway_received.is_some()) && self.streams.is_empty()
    }

    // ---- sending ----------------------------------------------------------------

    /// Opens a stream with a request (client).
    ///
    /// # Errors
    ///
    /// When the peer's concurrency limit is reached or it sent GOAWAY.
    pub fn open_stream<'a>(
        &mut self,
        fields: impl IntoIterator<Item = (&'a [u8], &'a [u8])>,
        end_stream: bool,
    ) -> Result<u32, H2Error> {
        debug_assert_eq!(self.role, Role::Client);
        if self.goaway_received.is_some() || self.failed.is_some() {
            return Err(Self::connection(
                ErrorCode::RefusedStream,
                "the connection is going away",
            ));
        }
        let open = self.streams.len() as u32;
        if open >= self.peer_max_streams {
            return Err(Self::connection(
                ErrorCode::RefusedStream,
                "the server's stream limit is reached",
            ));
        }
        let fields: Vec<(&[u8], &[u8])> = fields.into_iter().collect();
        let head = fields.iter().any(|&(n, v)| n == b":method" && v == b"HEAD");
        let stream = self.next_local_stream;
        self.next_local_stream += 2;
        self.streams.insert(
            stream,
            Stream {
                recv_closed: false,
                send_closed: end_stream,
                headers_received: false,
                send_window: self.peer_initial_window,
                recv_window: i64::from(self.config.stream_window),
                recv_released: 0,
                content_length: None,
                data_received: 0,
                pending: true,
                interim: false,
                head,
            },
        );
        let mut block = Vec::new();
        self.encoder.encode(fields, &mut block);
        encode_headers(
            &mut self.out,
            stream,
            &block,
            end_stream,
            self.peer_max_frame_size,
        );
        Ok(stream)
    }

    /// Sends a field section on `stream`: a response (server), or trailers
    /// (either role, with `end_stream`). Names must be lowercase, pseudo-header
    /// fields first.
    ///
    /// # Errors
    ///
    /// When the stream is closed for sending, or gone.
    pub fn send_headers<'a>(
        &mut self,
        stream: u32,
        fields: impl IntoIterator<Item = (&'a [u8], &'a [u8])>,
        end_stream: bool,
    ) -> Result<(), H2Error> {
        let Some(s) = self.streams.get_mut(&stream) else {
            return Err(H2Error::Stream {
                stream,
                code: ErrorCode::StreamClosed,
                why: "the stream is gone",
            });
        };
        if s.send_closed {
            return Err(H2Error::Stream {
                stream,
                code: ErrorCode::StreamClosed,
                why: "the stream is closed for sending",
            });
        }
        if end_stream {
            s.send_closed = true;
        }
        let mut block = Vec::new();
        self.encoder.encode(fields, &mut block);
        encode_headers(
            &mut self.out,
            stream,
            &block,
            end_stream,
            self.peer_max_frame_size,
        );
        if end_stream {
            self.finish_sending(stream);
        }
        Ok(())
    }

    /// How much of `stream`'s body may be sent now.
    #[must_use]
    pub fn send_capacity(&self, stream: u32) -> usize {
        self.streams
            .get(&stream)
            .filter(|s| !s.send_closed)
            .map_or(0, |s| s.send_window.min(self.send_window).max(0) as usize)
    }

    /// Sends as much of `data` on `stream` as the windows allow; returns how
    /// much. `end_stream` takes effect only when all of `data` was sent.
    ///
    /// # Errors
    ///
    /// When the stream is closed for sending, or gone.
    pub fn send_data(
        &mut self,
        stream: u32,
        data: &[u8],
        end_stream: bool,
    ) -> Result<usize, H2Error> {
        let capacity = self.send_capacity(stream);
        let Some(s) = self.streams.get_mut(&stream).filter(|s| !s.send_closed) else {
            return Err(H2Error::Stream {
                stream,
                code: ErrorCode::StreamClosed,
                why: "the stream is closed for sending",
            });
        };
        let n = capacity.min(data.len());
        let all = n == data.len();
        let max = self.peer_max_frame_size as usize;
        let mut chunks = data[..n].chunks(max).peekable();
        if n == 0 {
            if all && end_stream {
                encode_data(&mut self.out, stream, &[], true);
            }
        } else {
            while let Some(chunk) = chunks.next() {
                let last = chunks.peek().is_none();
                encode_data(&mut self.out, stream, chunk, last && all && end_stream);
            }
        }
        s.send_window -= n as i64;
        self.send_window -= n as i64;
        if all && end_stream {
            s.send_closed = true;
            self.finish_sending(stream);
        }
        Ok(n)
    }

    fn finish_sending(&mut self, stream: u32) {
        if let Some(s) = self.streams.get_mut(&stream) {
            // Answered: a reset now is no longer the Rapid Reset pattern.
            if self.role == Role::Server {
                s.pending = false;
            }
        }
        self.maybe_forget(stream);
    }

    /// Gives back `n` bytes of `stream`'s receive window (and the
    /// connection's), once the application has consumed them. WINDOW_UPDATE
    /// goes out when half a window has come back, not per call.
    pub fn release(&mut self, stream: u32, n: usize) {
        let n = n as u32;
        self.release_connection(n);
        let threshold = self.config.stream_window / 2;
        if let Some(s) = self.streams.get_mut(&stream)
            && !s.recv_closed
        {
            s.recv_released += n;
            if s.recv_released >= threshold {
                encode_window_update(&mut self.out, stream, s.recv_released);
                s.recv_window += i64::from(s.recv_released);
                s.recv_released = 0;
            }
        }
    }

    fn release_connection(&mut self, n: u32) {
        self.recv_released += n;
        if self.recv_released >= self.config.connection_window / 2 {
            encode_window_update(&mut self.out, 0, self.recv_released);
            self.recv_window += i64::from(self.recv_released);
            self.recv_released = 0;
        }
    }

    /// Resets `stream` from the application (a handler gave up, or a client
    /// abandoned the request).
    pub fn reset(&mut self, stream: u32, code: ErrorCode) {
        encode_rst_stream(&mut self.out, stream, code);
        self.streams.remove(&stream);
    }

    /// Whether the peer's first SETTINGS has arrived. A client waits for it
    /// before it counts on a connection's capacity.
    #[must_use]
    pub fn peer_settings_received(&self) -> bool {
        self.settings_received
    }

    /// The streams the peer allows open at once (`u32::MAX` until it says).
    #[must_use]
    pub fn peer_max_concurrent_streams(&self) -> u32 {
        self.peer_max_streams
    }

    /// Sends a PING (a keep-alive probe); its ACK arrives as ordinary input.
    pub fn ping(&mut self, data: [u8; 8]) {
        encode_ping(&mut self.out, false, data);
    }

    /// Begins a graceful shutdown: GOAWAY with the last stream seen; streams
    /// above it are refused, those below finish.
    pub fn go_away(&mut self, code: ErrorCode) {
        if self.goaway_sent.is_none() {
            encode_goaway(&mut self.out, self.last_peer_stream, code, &[]);
            self.goaway_sent = Some(self.last_peer_stream);
        }
    }

    /// The application is done with `stream` — answered it, abandoned it,
    /// or finished cleaning up after the peer reset it: it no longer counts
    /// toward the reset budget.
    pub fn done_with(&mut self, stream: u32) {
        self.pending_resets.remove(&stream);
        if let Some(s) = self.streams.get_mut(&stream) {
            s.pending = false;
        }
    }
}
