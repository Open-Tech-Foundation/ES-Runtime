//! HTTP/2 frames: what we write we read back, and every rule RFC 9113 gives a
//! single frame, with the reach of its error (stream or connection).

use super::frame::*;
use super::{ErrorCode, H2Error};

fn one(buf: &[u8]) -> Frame<'_> {
    match parse_frame(buf, DEFAULT_MAX_FRAME_SIZE) {
        Ok(Some((frame, used))) => {
            assert_eq!(used, buf.len(), "the whole frame is consumed");
            frame
        }
        other => panic!("{other:?}"),
    }
}

fn err(buf: &[u8]) -> H2Error {
    match parse_frame(buf, DEFAULT_MAX_FRAME_SIZE) {
        Err(e) => e,
        other => panic!("accepted: {other:?}"),
    }
}

/// A frame header followed by `payload`.
fn raw(kind: u8, flags: u8, stream: u32, payload: &[u8]) -> Vec<u8> {
    let len = (payload.len() as u32).to_be_bytes();
    let mut out = vec![len[1], len[2], len[3], kind, flags];
    out.extend_from_slice(&stream.to_be_bytes());
    out.extend_from_slice(payload);
    out
}

fn is_connection(e: &H2Error, code: ErrorCode) -> bool {
    matches!(e, H2Error::Connection { code: c, .. } if *c == code)
}

fn is_stream(e: &H2Error, stream: u32, code: ErrorCode) -> bool {
    matches!(e, H2Error::Stream { stream: s, code: c, .. } if *s == stream && *c == code)
}

#[test]
fn what_is_written_reads_back() {
    let mut out = Vec::new();
    encode_data(&mut out, 1, b"hello", true);
    assert_eq!(
        one(&out),
        Frame::Data {
            stream: 1,
            end_stream: true,
            data: b"hello",
            flow_len: 5
        }
    );

    out.clear();
    encode_settings(
        &mut out,
        &[
            Setting::MaxConcurrentStreams(256),
            Setting::InitialWindowSize(65_535),
        ],
    );
    assert_eq!(
        one(&out),
        Frame::Settings {
            ack: false,
            settings: vec![
                Setting::MaxConcurrentStreams(256),
                Setting::InitialWindowSize(65_535)
            ]
        }
    );
    out.clear();
    encode_settings_ack(&mut out);
    assert_eq!(
        one(&out),
        Frame::Settings {
            ack: true,
            settings: vec![]
        }
    );

    out.clear();
    encode_ping(&mut out, true, *b"12345678");
    assert_eq!(
        one(&out),
        Frame::Ping {
            ack: true,
            data: *b"12345678"
        }
    );
    out.clear();
    encode_goaway(&mut out, 7, ErrorCode::EnhanceYourCalm, b"calm");
    assert_eq!(
        one(&out),
        Frame::GoAway {
            last_stream: 7,
            code: ErrorCode::EnhanceYourCalm,
            debug: b"calm"
        }
    );
    out.clear();
    encode_rst_stream(&mut out, 3, ErrorCode::Cancel);
    assert_eq!(
        one(&out),
        Frame::RstStream {
            stream: 3,
            code: ErrorCode::Cancel
        }
    );
    out.clear();
    encode_window_update(&mut out, 0, 1 << 20);
    assert_eq!(
        one(&out),
        Frame::WindowUpdate {
            stream: 0,
            increment: 1 << 20
        }
    );
}

#[test]
fn a_long_field_block_is_split_into_continuations() {
    let block: Vec<u8> = (0..40_000u32).map(|i| i as u8).collect();
    let mut out = Vec::new();
    encode_headers(&mut out, 5, &block, true, DEFAULT_MAX_FRAME_SIZE);
    let mut at = 0;
    let mut joined = Vec::new();
    let mut kinds = Vec::new();
    while at < out.len() {
        let (frame, used) = parse_frame(&out[at..], DEFAULT_MAX_FRAME_SIZE)
            .unwrap()
            .unwrap();
        at += used;
        match frame {
            Frame::Headers {
                stream: 5,
                end_stream: true,
                end_headers,
                block,
            } => {
                kinds.push(("headers", end_headers));
                joined.extend_from_slice(block);
            }
            Frame::Continuation {
                stream: 5,
                end_headers,
                block,
            } => {
                kinds.push(("continuation", end_headers));
                joined.extend_from_slice(block);
            }
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(joined, block);
    assert_eq!(
        kinds,
        [
            ("headers", false),
            ("continuation", false),
            ("continuation", true)
        ]
    );
    // A short one is one HEADERS frame with END_HEADERS.
    out.clear();
    encode_headers(&mut out, 5, b"abc", false, DEFAULT_MAX_FRAME_SIZE);
    assert_eq!(
        one(&out),
        Frame::Headers {
            stream: 5,
            end_stream: false,
            end_headers: true,
            block: b"abc"
        }
    );
}

#[test]
fn a_partial_frame_waits_for_the_rest() {
    let mut out = Vec::new();
    encode_data(&mut out, 1, b"hello", false);
    for n in 0..out.len() {
        assert_eq!(
            parse_frame(&out[..n], DEFAULT_MAX_FRAME_SIZE),
            Ok(None),
            "prefix {n}"
        );
    }
}

#[test]
fn padding_is_removed_and_must_fit() {
    // DATA, PADDED, pad length 3: "hi" then 3 bytes of padding.
    let frame = raw(0x0, 0x8, 1, &[3, b'h', b'i', 0, 0, 0]);
    assert_eq!(
        one(&frame),
        Frame::Data {
            stream: 1,
            end_stream: false,
            data: b"hi",
            flow_len: 6
        }
    );
    // HEADERS, PADDED and PRIORITY: pad length, 5 priority bytes, the block, padding.
    let frame = raw(0x1, 0x8 | 0x20 | 0x4, 3, &[1, 0, 0, 0, 1, 16, b'x', 0]);
    assert_eq!(
        one(&frame),
        Frame::Headers {
            stream: 3,
            end_stream: false,
            end_headers: true,
            block: b"x"
        }
    );
    // Padding as long as the payload.
    assert!(is_connection(
        &err(&raw(0x0, 0x8, 1, &[5, 0, 0, 0, 0])),
        ErrorCode::ProtocolError
    ));
}

#[test]
fn frames_on_the_wrong_stream_end_the_connection() {
    for (kind, payload) in [
        (0x0u8, &b"x"[..]),
        (0x1, b""),
        (0x3, &[0, 0, 0, 0][..]),
        (0x5, &[0, 0, 0, 2][..]),
        (0x9, b""),
    ] {
        assert!(
            is_connection(&err(&raw(kind, 0, 0, payload)), ErrorCode::ProtocolError),
            "type {kind} on stream 0"
        );
    }
    for (kind, payload) in [
        (0x4u8, &b""[..]),
        (0x6, &[0u8; 8][..]),
        (0x7, &[0u8; 8][..]),
    ] {
        assert!(
            is_connection(&err(&raw(kind, 0, 1, payload)), ErrorCode::ProtocolError),
            "type {kind} on a stream"
        );
    }
}

#[test]
fn fixed_sizes_are_enforced_with_their_reach() {
    assert!(is_connection(
        &err(&raw(0x6, 0, 0, &[0; 7])),
        ErrorCode::FrameSizeError
    ));
    assert!(is_connection(
        &err(&raw(0x3, 0, 1, &[0; 3])),
        ErrorCode::FrameSizeError
    ));
    assert!(is_connection(
        &err(&raw(0x8, 0, 1, &[0; 5])),
        ErrorCode::FrameSizeError
    ));
    assert!(is_connection(
        &err(&raw(0x4, 0, 0, &[0; 5])),
        ErrorCode::FrameSizeError
    ));
    assert!(is_connection(
        &err(&raw(0x4, 0x1, 0, &[0; 6])),
        ErrorCode::FrameSizeError
    ));
    assert!(is_connection(
        &err(&raw(0x7, 0, 0, &[0; 7])),
        ErrorCode::FrameSizeError
    ));
    // PRIORITY of the wrong size is only its stream's problem.
    assert!(is_stream(
        &err(&raw(0x2, 0, 1, &[0; 4])),
        1,
        ErrorCode::FrameSizeError
    ));
}

#[test]
fn an_oversized_frame_ends_what_it_could_have_changed() {
    let big = vec![0u8; DEFAULT_MAX_FRAME_SIZE as usize + 1];
    assert!(is_stream(
        &err(&raw(0x0, 0, 1, &big)),
        1,
        ErrorCode::FrameSizeError
    ));
    assert!(is_connection(
        &err(&raw(0x1, 0, 1, &big)),
        ErrorCode::FrameSizeError
    ));
    assert!(is_connection(
        &err(&raw(0x9, 0, 1, &big)),
        ErrorCode::FrameSizeError
    ));
    assert!(is_connection(
        &err(&raw(0x0, 0, 0, &big)),
        ErrorCode::FrameSizeError
    ));
    // The limit is ours to raise.
    assert!(
        parse_frame(&raw(0x0, 0, 1, &big), DEFAULT_MAX_FRAME_SIZE * 2)
            .unwrap()
            .is_some()
    );
}

#[test]
fn settings_values_are_checked_and_unknown_ones_skipped() {
    let s = |id: u16, v: u32| {
        let mut p = id.to_be_bytes().to_vec();
        p.extend_from_slice(&v.to_be_bytes());
        raw(0x4, 0, 0, &p)
    };
    assert!(is_connection(&err(&s(0x2, 2)), ErrorCode::ProtocolError));
    assert!(is_connection(
        &err(&s(0x4, 1 << 31)),
        ErrorCode::FlowControlError
    ));
    assert!(is_connection(
        &err(&s(0x5, 16_383)),
        ErrorCode::ProtocolError
    ));
    assert!(is_connection(
        &err(&s(0x5, 1 << 24)),
        ErrorCode::ProtocolError
    ));
    assert_eq!(
        one(&s(0x5, 1 << 20)),
        Frame::Settings {
            ack: false,
            settings: vec![Setting::MaxFrameSize(1 << 20)]
        }
    );
    assert_eq!(
        one(&s(0x99, 7)),
        Frame::Settings {
            ack: false,
            settings: vec![]
        }
    );
}

#[test]
fn a_zero_window_increment_reaches_as_far_as_its_window() {
    assert!(is_connection(
        &err(&raw(0x8, 0, 0, &[0; 4])),
        ErrorCode::ProtocolError
    ));
    assert!(is_stream(
        &err(&raw(0x8, 0, 3, &[0; 4])),
        3,
        ErrorCode::ProtocolError
    ));
}

#[test]
fn a_stream_depending_on_itself_is_a_stream_error() {
    assert!(is_stream(
        &err(&raw(0x2, 0, 3, &[0, 0, 0, 3, 16])),
        3,
        ErrorCode::ProtocolError
    ));
    assert!(is_stream(
        &err(&raw(0x1, 0x20 | 0x4, 3, &[0, 0, 0, 3, 16])),
        3,
        ErrorCode::ProtocolError
    ));
}

#[test]
fn unknown_frames_and_codes_pass_through() {
    assert_eq!(
        one(&raw(0xfa, 0xff, 9, b"anything")),
        Frame::Unknown {
            kind: 0xfa,
            stream: 9
        }
    );
    let frame = raw(0x3, 0, 1, &0xdead_u32.to_be_bytes());
    assert_eq!(
        one(&frame),
        Frame::RstStream {
            stream: 1,
            code: ErrorCode::Unknown(0xdead)
        }
    );
    // The reserved bit of the stream id is ignored.
    let mut frame = raw(0x0, 0, 1, b"x");
    frame[5] |= 0x80;
    assert!(matches!(one(&frame), Frame::Data { stream: 1, .. }));
}
