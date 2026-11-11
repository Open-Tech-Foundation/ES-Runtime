//! The HTTP/2 connection state machine: a client and a server talking to each
//! other in memory, and a server fed hand-made frames for every rule and every
//! bound.

use super::ErrorCode;
use super::conn::{Config, Connection, Event};
use super::frame::{
    Frame, PREFACE, encode_data, encode_headers, encode_ping, encode_rst_stream, encode_settings,
    encode_window_update, parse_frame,
};
use crate::hpack::Encoder;

/// Moves bytes both ways until neither side has anything to say.
fn pump(client: &mut Connection, server: &mut Connection) {
    for _ in 0..1000 {
        let a = client.output().to_vec();
        client.consume_output(a.len());
        let b = server.output().to_vec();
        server.consume_output(b.len());
        if a.is_empty() && b.is_empty() {
            return;
        }
        if !a.is_empty() {
            let used = server.receive(&a).expect("server");
            assert_eq!(used, a.len());
        }
        if !b.is_empty() {
            let used = client.receive(&b).expect("client");
            assert_eq!(used, b.len());
        }
    }
    panic!("the two sides never went quiet");
}

/// What the application would act on: everything but notices that more may
/// be sent, which the flow-control test looks at on its own.
fn events(c: &mut Connection) -> Vec<Event> {
    std::iter::from_fn(|| c.poll_event())
        .filter(|e| !matches!(e, Event::SendCapacity { .. }))
        .collect()
}

fn get(path: &[u8]) -> Vec<(&[u8], &[u8])> {
    vec![
        (b":method", b"GET"),
        (b":scheme", b"https"),
        (b":authority", b"example.test"),
        (b":path", path),
    ]
}

#[test]
fn a_request_and_its_response_cross_the_connection() {
    let mut client = Connection::client(Config::default());
    let mut server = Connection::server(Config::default());
    pump(&mut client, &mut server);
    let stream = client.open_stream(get(b"/hello"), true).unwrap();
    pump(&mut client, &mut server);
    let got = events(&mut server);
    let [
        Event::Headers {
            stream: s,
            pseudo,
            fields,
            end_stream: true,
        },
    ] = got.as_slice()
    else {
        panic!("{got:?}");
    };
    assert_eq!(*s, stream);
    assert_eq!(pseudo.path.as_deref(), Some(&b"/hello"[..]));
    assert!(fields.is_empty());

    server
        .send_headers(
            stream,
            [
                (&b":status"[..], &b"200"[..]),
                (b"content-type", b"text/plain"),
            ],
            false,
        )
        .unwrap();
    assert_eq!(server.send_data(stream, b"hello", true).unwrap(), 5);
    pump(&mut client, &mut server);
    let got = events(&mut client);
    assert!(
        matches!(&got[0], Event::Headers { pseudo, end_stream: false, .. } if pseudo.status == Some(200))
    );
    assert_eq!(
        got[1],
        Event::Data {
            stream,
            data: b"hello".to_vec(),
            end_stream: true
        }
    );
    assert_eq!(got.len(), 2);
}

#[test]
fn bodies_and_trailers_go_both_ways() {
    let mut client = Connection::client(Config::default());
    let mut server = Connection::server(Config::default());
    let mut post = get(b"/upload");
    post[0].1 = b"POST";
    let stream = client.open_stream(post, false).unwrap();
    client.send_data(stream, b"part one, ", false).unwrap();
    client.send_data(stream, b"part two", false).unwrap();
    client
        .send_headers(stream, [(&b"x-sum"[..], &b"42"[..])], true)
        .unwrap();
    pump(&mut client, &mut server);
    let got = events(&mut server);
    let body: Vec<u8> = got
        .iter()
        .filter_map(|e| {
            if let Event::Data { data, .. } = e {
                Some(data.clone())
            } else {
                None
            }
        })
        .flatten()
        .collect();
    assert_eq!(body, b"part one, part two");
    assert_eq!(
        got.last(),
        Some(&Event::Trailers {
            stream,
            fields: vec![(b"x-sum".to_vec(), b"42".to_vec())]
        })
    );
    server
        .send_headers(stream, [(&b":status"[..], &b"201"[..])], false)
        .unwrap();
    server
        .send_headers(stream, [(&b"grpc-status"[..], &b"0"[..])], true)
        .unwrap();
    pump(&mut client, &mut server);
    let got = events(&mut client);
    assert!(
        matches!(got.last(), Some(Event::Trailers { .. })),
        "{got:?}"
    );
}

/// A body larger than every window moves only as the receiver releases what
/// it read, and nothing overruns.
#[test]
fn flow_control_holds_a_large_body_to_the_windows() {
    let mut client = Connection::client(Config::default());
    let mut server = Connection::server(Config::default());
    pump(&mut client, &mut server);
    let mut post = get(b"/big");
    post[0].1 = b"POST";
    let stream = client.open_stream(post, false).unwrap();
    pump(&mut client, &mut server);
    let body = vec![7u8; 5 << 20];
    let mut sent = 0;
    let mut received = 0;
    let mut rounds = 0;
    while received < body.len() {
        rounds += 1;
        assert!(rounds < 1000, "stalled at {sent} sent, {received} received");
        if sent < body.len() {
            sent += client.send_data(stream, &body[sent..], true).unwrap();
        }
        pump(&mut client, &mut server);
        for e in events(&mut server) {
            if let Event::Data { data, .. } = e {
                received += data.len();
                server.release(stream, data.len());
            }
        }
        // The client's window grows only as the server releases.
        assert!(
            sent - received <= 2 << 20,
            "more in flight than the connection window"
        );
        events(&mut client);
    }
    assert_eq!(received, body.len());
    assert!(rounds > 2, "the body should have needed several windows");
}

#[test]
fn a_field_block_larger_than_a_frame_crosses_as_continuations() {
    let mut client = Connection::client(Config::default());
    let mut server = Connection::server(Config {
        max_header_list_size: 1 << 20,
        ..Config::default()
    });
    pump(&mut client, &mut server);
    let big = vec![b'v'; 40_000];
    let mut req = get(b"/");
    req.push((b"x-big", &big));
    client.open_stream(req, true).unwrap();
    pump(&mut client, &mut server);
    let got = events(&mut server);
    assert!(
        matches!(&got[..], [Event::Headers { fields, .. }] if fields[0].1.len() == 40_000),
        "{got:?}"
    );
}

// ---- a server fed frames by hand --------------------------------------------

struct Peer {
    server: Connection,
    hpack: Encoder,
}

impl Peer {
    fn new(config: Config) -> Self {
        let mut server = Connection::server(config);
        let mut hello = PREFACE.to_vec();
        encode_settings(&mut hello, &[]);
        server.receive(&hello).unwrap();
        let n = server.output().len();
        server.consume_output(n);
        Peer {
            server,
            hpack: Encoder::new(4096),
        }
    }

    fn headers(&mut self, stream: u32, fields: &[(&[u8], &[u8])], end_stream: bool) -> Vec<u8> {
        let mut block = Vec::new();
        self.hpack.encode(fields.iter().copied(), &mut block);
        let mut out = Vec::new();
        encode_headers(&mut out, stream, &block, end_stream, 16_384);
        out
    }

    fn send(&mut self, bytes: &[u8]) -> Result<usize, super::H2Error> {
        self.server.receive(bytes)
    }

    /// The frames the server wrote back, as (type, stream, code).
    fn replies(&mut self) -> Vec<(&'static str, u32, Option<ErrorCode>)> {
        let out = self.server.output().to_vec();
        self.server.consume_output(out.len());
        let mut at = 0;
        let mut seen = Vec::new();
        while let Ok(Some((frame, used))) = parse_frame(&out[at..], 16_384) {
            at += used;
            seen.push(match frame {
                Frame::GoAway { code, .. } => ("goaway", 0, Some(code)),
                Frame::RstStream { stream, code } => ("rst", stream, Some(code)),
                Frame::Headers { stream, .. } => ("headers", stream, None),
                Frame::Settings { .. } => ("settings", 0, None),
                Frame::Ping { .. } => ("ping", 0, None),
                Frame::WindowUpdate { stream, .. } => ("window", stream, None),
                Frame::Data { stream, .. } => ("data", stream, None),
                _ => ("other", 0, None),
            });
        }
        seen
    }
}

fn goaway(code: ErrorCode) -> (&'static str, u32, Option<ErrorCode>) {
    ("goaway", 0, Some(code))
}

#[test]
fn without_the_preface_the_connection_ends() {
    let mut server = Connection::server(Config::default());
    assert!(server.receive(b"GET / HTTP/1.1\r\n\r\n").is_err());
}

#[test]
fn stream_rules_end_the_connection_or_the_stream_as_rfc_9113_says() {
    let mut p = Peer::new(Config::default());
    let mut data = Vec::new();
    encode_data(&mut data, 3, b"x", false);
    assert!(p.send(&data).is_err(), "DATA on an idle stream");
    assert_eq!(p.replies().last(), Some(&goaway(ErrorCode::ProtocolError)));

    let mut p = Peer::new(Config::default());
    let frame = p.headers(2, &get(b"/"), true);
    assert!(p.send(&frame).is_err(), "a client opened an even stream");

    // A malformed request resets its stream and the connection carries on.
    let mut p = Peer::new(Config::default());
    let mut bad = get(b"/");
    bad.push((b"X-Upper", b"v"));
    let frame = p.headers(1, &bad, true);
    p.send(&frame).unwrap();
    assert_eq!(p.replies(), [("rst", 1, Some(ErrorCode::ProtocolError))]);
    let frame = p.headers(3, &get(b"/"), true);
    p.send(&frame).unwrap();
    assert!(matches!(
        p.server.poll_event(),
        Some(Event::Headers { stream: 3, .. })
    ));

    // A stream id going backwards is a connection error.
    let frame = p.headers(1, &get(b"/"), true);
    assert!(p.send(&frame).is_err());
}

#[test]
fn each_malformed_request_is_its_streams_error() {
    let cases: &[&[(&[u8], &[u8])]] = &[
        &[(b":method", b"GET"), (b":path", b"/")],
        &[
            (b":method", b"GET"),
            (b":scheme", b"https"),
            (b":path", b""),
        ],
        &[
            (b":method", b"GET"),
            (b":scheme", b"https"),
            (b":path", b"/"),
            (b":status", b"200"),
        ],
        &[
            (b":method", b"GET"),
            (b":scheme", b"https"),
            (b"a", b"b"),
            (b":path", b"/"),
        ],
        &[
            (b":method", b"GET"),
            (b":method", b"GET"),
            (b":scheme", b"https"),
            (b":path", b"/"),
        ],
        &[
            (b":method", b"GET"),
            (b":scheme", b"https"),
            (b":path", b"/"),
            (b"connection", b"close"),
        ],
        &[
            (b":method", b"GET"),
            (b":scheme", b"https"),
            (b":path", b"/"),
            (b"te", b"gzip"),
        ],
        &[
            (b":method", b"GET"),
            (b":scheme", b"https"),
            (b":path", b"/"),
            (b"x", b" padded"),
        ],
        &[
            (b":method", b"CONNECT"),
            (b":authority", b"h:443"),
            (b":path", b"/"),
        ],
    ];
    for (i, fields) in cases.iter().enumerate() {
        let mut p = Peer::new(Config::default());
        let frame = p.headers(1, fields, true);
        p.send(&frame).unwrap();
        assert_eq!(
            p.replies(),
            [("rst", 1, Some(ErrorCode::ProtocolError))],
            "case {i}"
        );
        assert!(
            p.server.poll_event().is_none(),
            "case {i} reached the application"
        );
    }
}

#[test]
fn content_length_must_match_the_data() {
    let mut p = Peer::new(Config::default());
    let mut post = get(b"/");
    post[0].1 = b"POST";
    post.push((b"content-length", b"10"));
    let mut frames = p.headers(1, &post, false);
    encode_data(&mut frames, 1, b"short", true);
    p.send(&frames).unwrap();
    assert!(
        p.replies()
            .contains(&("rst", 1, Some(ErrorCode::ProtocolError)))
    );
}

#[test]
fn the_concurrency_limit_refuses_the_stream_past_it() {
    let mut p = Peer::new(Config {
        max_concurrent_streams: 2,
        ..Config::default()
    });
    for stream in [1, 3, 5] {
        let frame = p.headers(stream, &get(b"/"), false);
        p.send(&frame).unwrap();
    }
    assert_eq!(p.replies(), [("rst", 5, Some(ErrorCode::RefusedStream))]);
}

/// CVE-2023-44487: streams opened and reset while the application is still
/// on them are bounded; streams the application has finished with are not.
#[test]
fn rapid_reset_is_bounded_but_ordinary_cancels_are_not() {
    let mut p = Peer::new(Config {
        max_pending_resets: 10,
        ..Config::default()
    });
    let mut result = Ok(0);
    for i in 0..20 {
        let stream = 1 + 2 * i;
        let mut frames = p.headers(stream, &get(b"/"), true);
        encode_rst_stream(&mut frames, stream, ErrorCode::Cancel);
        result = p.send(&frames);
        if result.is_err() {
            break;
        }
    }
    assert!(result.is_err());
    assert_eq!(
        p.replies().last(),
        Some(&goaway(ErrorCode::EnhanceYourCalm))
    );

    let mut p = Peer::new(Config {
        max_pending_resets: 10,
        ..Config::default()
    });
    for i in 0..200 {
        let stream = 1 + 2 * i;
        let mut frames = p.headers(stream, &get(b"/"), true);
        encode_rst_stream(&mut frames, stream, ErrorCode::Cancel);
        p.send(&frames).unwrap();
        // The application notices and lets the stream go.
        p.server.done_with(stream);
    }
}

#[test]
fn a_continuation_flood_ends_the_connection() {
    let mut p = Peer::new(Config::default());
    let mut block = Vec::new();
    p.hpack.encode(get(b"/"), &mut block);
    // HEADERS without END_HEADERS, then CONTINUATIONs that never end.
    let mut frames = vec![0, 0, 1, 0x1, 0, 0, 0, 0, 1, block[0]];
    for _ in 0..100 {
        frames.extend_from_slice(&[0, 0, 1, 0x9, 0, 0, 0, 0, 1, 0x80 | 1]);
    }
    assert!(p.send(&frames).is_err());
    assert_eq!(
        p.replies().last(),
        Some(&goaway(ErrorCode::EnhanceYourCalm))
    );
}

#[test]
fn a_frame_may_not_interrupt_a_field_block() {
    let mut p = Peer::new(Config::default());
    let mut frames = vec![0, 0, 1, 0x1, 0, 0, 0, 0, 1, 0x82];
    encode_ping(&mut frames, false, [0; 8]);
    assert!(p.send(&frames).is_err());
    assert_eq!(p.replies().last(), Some(&goaway(ErrorCode::ProtocolError)));
}

#[test]
fn a_header_list_past_its_limit_is_answered_431() {
    let mut p = Peer::new(Config {
        max_header_list_size: 1024,
        ..Config::default()
    });
    let big = vec![b'v'; 2000];
    let mut req = get(b"/");
    req.push((b"x-big", &big));
    let frame = p.headers(1, &req, true);
    p.send(&frame).unwrap();
    assert_eq!(p.replies(), [("headers", 1, None)]);
    assert!(p.server.poll_event().is_none());
}

#[test]
fn ping_and_empty_frame_floods_end_the_connection() {
    // PINGs whose ACKs are never read.
    let mut p = Peer::new(Config::default());
    let mut pings = Vec::new();
    for _ in 0..70_000 {
        encode_ping(&mut pings, false, [1; 8]);
    }
    assert!(p.send(&pings).is_err());
    assert!(
        p.server
            .output()
            .ends_with(b"replies owed faster than read")
            || p.server.is_failed()
    );

    let mut p = Peer::new(Config::default());
    let mut frames = p.headers(
        1,
        &[
            (b":method", b"POST"),
            (b":scheme", b"https"),
            (b":path", b"/"),
        ],
        false,
    );
    for _ in 0..200 {
        encode_data(&mut frames, 1, b"", false);
    }
    assert!(p.send(&frames).is_err());
    assert_eq!(
        p.replies().last(),
        Some(&goaway(ErrorCode::EnhanceYourCalm))
    );
}

#[test]
fn a_window_pushed_past_2_31_is_a_flow_control_error() {
    let mut p = Peer::new(Config::default());
    let mut frames = Vec::new();
    encode_window_update(&mut frames, 0, 0x7fff_ffff);
    assert!(p.send(&frames).is_err());
    assert_eq!(
        p.replies().last(),
        Some(&goaway(ErrorCode::FlowControlError))
    );

    let mut p = Peer::new(Config::default());
    let mut frames = p.headers(1, &get(b"/"), false);
    encode_window_update(&mut frames, 1, 0x7fff_ffff);
    p.send(&frames).unwrap();
    assert!(
        p.replies()
            .contains(&("rst", 1, Some(ErrorCode::FlowControlError)))
    );
}

#[test]
fn a_graceful_goaway_refuses_streams_it_did_not_cover() {
    let mut p = Peer::new(Config::default());
    let frame = p.headers(1, &get(b"/"), true);
    p.send(&frame).unwrap();
    p.server.go_away(ErrorCode::NoError);
    let frame = p.headers(3, &get(b"/"), true);
    p.send(&frame).unwrap();
    let replies = p.replies();
    assert_eq!(replies[0], goaway(ErrorCode::NoError));
    assert!(replies.contains(&("rst", 3, Some(ErrorCode::RefusedStream))));
}
