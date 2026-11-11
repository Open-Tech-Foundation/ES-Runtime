//! HTTP/1.1 parser, decoder and encoder: the rules D217 writes down, one test
//! per rule, plus incremental feeding (a byte at a time must agree with all at
//! once) and the pipelining boundaries.

use super::*;
use crate::fields::Field;

fn parse(raw: &[u8]) -> Result<Parsed, RequestError> {
    let mut fields = Vec::new();
    parse_request(raw, &Limits::default(), &mut fields)
}

fn complete(raw: &[u8]) -> (RequestHead, Vec<(String, String)>) {
    let mut fields: Vec<Field> = Vec::new();
    match parse_request(raw, &Limits::default(), &mut fields) {
        Ok(Parsed::Complete(head)) => {
            let pairs = fields
                .iter()
                .map(|f| {
                    (
                        String::from_utf8_lossy(f.name.of(raw)).into_owned(),
                        String::from_utf8_lossy(f.value.of(raw)).into_owned(),
                    )
                })
                .collect();
            (head, pairs)
        }
        other => panic!("{:?} → {other:?}", String::from_utf8_lossy(raw)),
    }
}

fn refused(raw: &[u8]) -> RequestError {
    match parse(raw) {
        Err(e) => e,
        other => panic!("{:?} was accepted: {other:?}", String::from_utf8_lossy(raw)),
    }
}

// ---- well-formed requests -------------------------------------------------

#[test]
fn a_simple_get_parses_to_spans_into_the_buffer() {
    let raw = b"GET /a?b=1 HTTP/1.1\r\nHost: example.test\r\nX-Two:  v a l \t\r\n\r\n";
    let (head, fields) = complete(raw);
    assert_eq!(head.method.of(raw), b"GET");
    assert_eq!(head.target.of(raw), b"/a?b=1");
    assert_eq!(head.target_form, TargetForm::Origin);
    assert_eq!(head.host.of(raw), b"example.test");
    assert_eq!(head.body, Body::Empty);
    assert!(head.keep_alive);
    assert_eq!(head.len, raw.len());
    assert_eq!(
        fields,
        [
            ("Host".into(), "example.test".into()),
            ("X-Two".into(), "v a l".into())
        ]
    );
}

#[test]
fn bodies_are_framed_by_length_or_chunked() {
    let (head, _) = complete(b"POST / HTTP/1.1\r\nHost: h\r\nContent-Length: 12\r\n\r\n");
    assert_eq!(head.body, Body::Length(12));
    let (head, _) = complete(b"POST / HTTP/1.1\r\nHost: h\r\nContent-Length: 0\r\n\r\n");
    assert_eq!(head.body, Body::Empty);
    let (head, _) = complete(b"POST / HTTP/1.1\r\nHost: h\r\nTransfer-Encoding: Chunked\r\n\r\n");
    assert_eq!(head.body, Body::Chunked);
    // Equal repeats of Content-Length are one length.
    let (head, _) =
        complete(b"POST / HTTP/1.1\r\nHost: h\r\nContent-Length: 5\r\ncontent-length: 5\r\n\r\n");
    assert_eq!(head.body, Body::Length(5));
}

#[test]
fn connection_expect_and_upgrade_are_read() {
    let (head, _) = complete(b"GET / HTTP/1.1\r\nHost: h\r\nConnection: keep-alive, Close\r\n\r\n");
    assert!(!head.keep_alive);
    let (head, _) =
        complete(b"PUT / HTTP/1.1\r\nHost: h\r\nExpect: 100-Continue\r\nContent-Length: 1\r\n\r\n");
    assert!(head.expect_continue);
    let (head, _) = complete(
        b"GET /ws HTTP/1.1\r\nHost: h\r\nConnection: Upgrade\r\nUpgrade: websocket\r\n\r\n",
    );
    assert!(head.upgrade);
    // An Upgrade field without the Connection option is not a request to switch.
    let (head, _) = complete(b"GET / HTTP/1.1\r\nHost: h\r\nUpgrade: websocket\r\n\r\n");
    assert!(!head.upgrade);
}

#[test]
fn target_forms_follow_their_methods() {
    assert_eq!(
        complete(b"OPTIONS * HTTP/1.1\r\nHost: h\r\n\r\n")
            .0
            .target_form,
        TargetForm::Asterisk
    );
    assert_eq!(
        complete(b"CONNECT h:443 HTTP/1.1\r\nHost: h:443\r\n\r\n")
            .0
            .target_form,
        TargetForm::Authority
    );
    assert_eq!(
        complete(b"GET http://h/x HTTP/1.1\r\nHost: h\r\n\r\n")
            .0
            .target_form,
        TargetForm::Absolute
    );
    for raw in [
        &b"GET * HTTP/1.1\r\nHost: h\r\n\r\n"[..],
        b"CONNECT /x HTTP/1.1\r\nHost: h\r\n\r\n",
        b"GET x HTTP/1.1\r\nHost: h\r\n\r\n",
        b"GET 1http://h/ HTTP/1.1\r\nHost: h\r\n\r\n",
    ] {
        assert!(
            matches!(refused(raw), RequestError::Malformed(_)),
            "{raw:?}"
        );
    }
}

#[test]
fn up_to_a_few_blank_lines_before_the_request_are_ignored() {
    let raw = b"\r\n\r\nGET / HTTP/1.1\r\nHost: h\r\n\r\n";
    let (head, _) = complete(raw);
    assert_eq!(head.method.of(raw), b"GET");
    assert_eq!(head.len, raw.len());
    assert!(matches!(
        refused(b"\nGET / HTTP/1.1\r\nHost: h\r\n\r\n"),
        RequestError::Malformed(_)
    ));
}

#[test]
fn an_empty_host_is_allowed() {
    let (head, _) = complete(b"GET / HTTP/1.1\r\nHost:\r\n\r\n");
    assert!(head.host.is_empty());
}

// ---- smuggling and framing ------------------------------------------------

#[test]
fn both_framings_at_once_are_refused() {
    let e = refused(
        b"POST / HTTP/1.1\r\nHost: h\r\nContent-Length: 3\r\nTransfer-Encoding: chunked\r\n\r\n",
    );
    assert_eq!(e.status(), 400);
}

#[test]
fn content_length_must_be_one_plain_number() {
    for cl in [
        "1, 1",
        "+1",
        "-1",
        " ",
        "0x10",
        "1 1",
        "18446744073709551616",
        "1.0",
    ] {
        let raw = format!("POST / HTTP/1.1\r\nHost: h\r\nContent-Length: {cl}\r\n\r\n");
        assert_eq!(refused(raw.as_bytes()).status(), 400, "{cl:?}");
    }
    let e =
        refused(b"POST / HTTP/1.1\r\nHost: h\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\n");
    assert_eq!(e, RequestError::Malformed("conflicting Content-Length"));
}

#[test]
fn chunked_must_be_the_final_coding_and_appear_once() {
    for te in [
        "gzip",
        "chunked, gzip",
        "chunked, chunked",
        "identity",
        ",",
        "chun ked",
    ] {
        let raw = format!("POST / HTTP/1.1\r\nHost: h\r\nTransfer-Encoding: {te}\r\n\r\n");
        assert_eq!(refused(raw.as_bytes()).status(), 400, "{te:?}");
    }
    // Split across two fields, the same rules hold.
    let raw = b"POST / HTTP/1.1\r\nHost: h\r\nTransfer-Encoding: chunked\r\nTransfer-Encoding: gzip\r\n\r\n";
    assert_eq!(refused(raw).status(), 400);
    // A coding we do not decode, under chunked, is "not implemented".
    let raw = b"POST / HTTP/1.1\r\nHost: h\r\nTransfer-Encoding: gzip, chunked\r\n\r\n";
    assert_eq!(refused(raw), RequestError::UnsupportedTransferCoding);
    assert_eq!(RequestError::UnsupportedTransferCoding.status(), 501);
}

#[test]
fn host_must_appear_exactly_once_and_be_an_authority() {
    assert_eq!(
        refused(b"GET / HTTP/1.1\r\n\r\n"),
        RequestError::Malformed("missing Host")
    );
    assert_eq!(
        refused(b"GET / HTTP/1.1\r\nHost: a\r\nHost: b\r\n\r\n"),
        RequestError::Malformed("more than one Host")
    );
    assert_eq!(
        refused(b"GET / HTTP/1.1\r\nHost: a/b\r\n\r\n"),
        RequestError::Malformed("invalid Host")
    );
}

#[test]
fn line_endings_must_be_crlf() {
    for raw in [
        &b"GET / HTTP/1.1\nHost: h\r\n\r\n"[..],
        b"GET / HTTP/1.1\r\nHost: h\n\r\n",
        b"GET / HTTP/1.1\r\nHost: h\r\n\n",
        b"GET / HTTP/1.1\rHost: h\r\n\r\n",
        b"GET / HTTP/1.1\r\nX: a\rb\r\nHost: h\r\n\r\n",
    ] {
        assert!(
            matches!(refused(raw), RequestError::Malformed(_)),
            "{raw:?}"
        );
    }
}

#[test]
fn obs_fold_and_whitespace_before_a_colon_are_refused() {
    assert_eq!(
        refused(b"GET / HTTP/1.1\r\nHost: h\r\nX: a\r\n b\r\n\r\n"),
        RequestError::Malformed("obs-fold")
    );
    assert_eq!(
        refused(b"GET / HTTP/1.1\r\nHost : h\r\n\r\n"),
        RequestError::Malformed("invalid field name")
    );
    assert_eq!(
        refused(b"GET / HTTP/1.1\r\n: x\r\nHost: h\r\n\r\n"),
        RequestError::Malformed("empty field name")
    );
}

#[test]
fn controls_in_values_and_targets_are_refused() {
    for raw in [
        &b"GET / HTTP/1.1\r\nHost: h\r\nX: a\x00b\r\n\r\n"[..],
        b"GET / HTTP/1.1\r\nHost: h\r\nX: a\x7fb\r\n\r\n",
        b"GET /a\x01 HTTP/1.1\r\nHost: h\r\n\r\n",
        b"GET /\xc3\xa9 HTTP/1.1\r\nHost: h\r\n\r\n",
        b"GET  / HTTP/1.1\r\nHost: h\r\n\r\n",
        b"GET / HTTP/1.1 \r\nHost: h\r\n\r\n",
        b"G\x01T / HTTP/1.1\r\nHost: h\r\n\r\n",
    ] {
        assert!(
            matches!(refused(raw), RequestError::Malformed(_)),
            "{raw:?}"
        );
    }
    // obs-text in a value is allowed.
    complete(b"GET / HTTP/1.1\r\nHost: h\r\nX: caf\xc3\xa9\r\n\r\n");
}

#[test]
fn only_http_1_1_is_spoken() {
    for v in ["HTTP/1.0", "HTTP/2.0", "HTTP/0.9", "HTTP/9.9"] {
        let raw = format!("GET / {v}\r\nHost: h\r\n\r\n");
        assert_eq!(
            refused(raw.as_bytes()),
            RequestError::VersionNotSupported,
            "{v}"
        );
    }
    assert_eq!(RequestError::VersionNotSupported.status(), 505);
    for v in ["HTTP/1.1.1", "http/1.1", "HTTP/11", "HTTP/1.x"] {
        let raw = format!("GET / {v}\r\nHost: h\r\n\r\n");
        assert!(
            matches!(refused(raw.as_bytes()), RequestError::Malformed(_)),
            "{v}"
        );
    }
}

#[test]
fn an_upgrade_with_a_body_is_refused() {
    let raw = b"GET / HTTP/1.1\r\nHost: h\r\nConnection: upgrade\r\nUpgrade: websocket\r\nContent-Length: 3\r\n\r\n";
    assert!(matches!(refused(raw), RequestError::Malformed(_)));
}

#[test]
fn an_expectation_other_than_continue_is_417() {
    let e = refused(b"GET / HTTP/1.1\r\nHost: h\r\nExpect: 200-ok\r\n\r\n");
    assert_eq!(e.status(), 417);
}

// ---- limits -----------------------------------------------------------------

#[test]
fn limits_give_their_own_statuses() {
    let limits = Limits {
        max_head_bytes: 64,
        max_fields: 2,
        max_target_bytes: 8,
    };
    let mut fields = Vec::new();
    let big = format!("GET / HTTP/1.1\r\nHost: h\r\nX: {}\r\n\r\n", "a".repeat(64));
    assert_eq!(
        parse_request(big.as_bytes(), &limits, &mut fields),
        Err(RequestError::HeadTooLarge)
    );
    // Still incomplete at the limit is also too large, not partial.
    let open = format!("GET / HTTP/1.1\r\nX: {}", "a".repeat(64));
    assert_eq!(
        parse_request(open.as_bytes(), &limits, &mut fields),
        Err(RequestError::HeadTooLarge)
    );
    let many = b"GET / HTTP/1.1\r\nHost: h\r\nA: 1\r\nB: 2\r\n\r\n";
    assert_eq!(
        parse_request(many, &limits, &mut fields),
        Err(RequestError::TooManyFields)
    );
    let long = b"GET /123456789 HTTP/1.1\r\nHost: h\r\n\r\n";
    assert_eq!(
        parse_request(long, &limits, &mut fields),
        Err(RequestError::TargetTooLong)
    );
    assert_eq!(RequestError::TooManyFields.status(), 431);
    assert_eq!(RequestError::TargetTooLong.status(), 414);
}

// ---- incremental parsing --------------------------------------------------

#[test]
fn every_prefix_of_a_good_head_is_partial() {
    let raw = b"\r\nPOST /x HTTP/1.1\r\nHost: h\r\nTransfer-Encoding: chunked\r\nX: y\r\n\r\n";
    for n in 0..raw.len() {
        assert_eq!(parse(&raw[..n]), Ok(Parsed::Partial), "prefix {n}");
    }
    assert!(matches!(parse(raw), Ok(Parsed::Complete(_))));
}

#[test]
fn a_bad_byte_is_refused_before_the_head_completes() {
    assert!(matches!(
        parse(b"GET / HTTP/1.1\nHo"),
        Err(RequestError::Malformed(_))
    ));
}

#[test]
fn pipelined_requests_parse_one_at_a_time() {
    let raw = b"GET /1 HTTP/1.1\r\nHost: h\r\n\r\nGET /2 HTTP/1.1\r\nHost: h\r\n\r\n";
    let (first, _) = complete(raw);
    assert_eq!(first.target.of(raw), b"/1");
    let rest = &raw[first.len..];
    let (second, _) = complete(rest);
    assert_eq!(second.target.of(rest), b"/2");
    assert_eq!(second.len, rest.len());
}

// ---- chunked ------------------------------------------------------------

/// Decodes `body` fed `step` bytes at a time; returns the data, the trailers
/// and how many bytes the body took.
type Dechunked = (Vec<u8>, Vec<(String, String)>, usize);

fn dechunk(body: &[u8], step: usize) -> Result<Dechunked, ChunkError> {
    let mut d = ChunkedDecoder::new(Limits::default());
    let mut data = Vec::new();
    let mut offset = 0;
    let mut available = step.min(body.len());
    loop {
        let (used, chunk) = d.decode(&body[offset..available])?;
        offset += used;
        match chunk {
            Chunk::Data(bytes) => data.extend_from_slice(bytes),
            Chunk::End => break,
            Chunk::NeedMore => {
                assert!(available < body.len(), "ran out of input before the end");
                available = (available + step).min(body.len());
            }
        }
    }
    let trailers = d
        .trailers()
        .map(|(n, v)| {
            (
                String::from_utf8_lossy(n).into_owned(),
                String::from_utf8_lossy(v).into_owned(),
            )
        })
        .collect();
    Ok((data, trailers, offset))
}

#[test]
fn a_chunked_body_decodes_the_same_whatever_the_read_sizes() {
    let body = b"5\r\nhello\r\n1;ext=\"v\"\r\n \r\nA\r\n0123456789\r\n0\r\nX-Sum: 1\r\nY:  2 \r\n\r\nGET /next";
    for step in [1, 2, 3, 7, body.len()] {
        let (data, trailers, used) = dechunk(body, step).unwrap();
        assert_eq!(data, b"hello 0123456789", "step {step}");
        assert_eq!(
            trailers,
            [("X-Sum".into(), "1".into()), ("Y".into(), "2".into())],
            "step {step}"
        );
        // It stops at the end of the message, leaving the next request.
        assert_eq!(&body[used..], b"GET /next", "step {step}");
    }
}

#[test]
fn an_empty_chunked_body_is_just_the_last_chunk() {
    let (data, trailers, used) = dechunk(b"0\r\n\r\n", 1).unwrap();
    assert!(data.is_empty() && trailers.is_empty());
    assert_eq!(used, 5);
}

#[test]
fn chunked_framing_errors_are_refused() {
    for body in [
        &b"\r\n"[..],               // no size
        b"g\r\n",                   // not hex
        b"5\nhello\r\n0\r\n\r\n",   // bare LF after the size
        b"5\r\nhelloX\r\n",         // data longer than the size
        b"5\r\nhello\n0\r\n\r\n",   // bare LF after the data
        b"11111111111111111\r\n",   // 17 hex digits
        b"1;a\x01\r\nx\r\n",        // control in an extension
        b"0\r\nX : y\r\n\r\n",      // whitespace before a trailer's colon
        b"0\r\nX: y\r\n z\r\n\r\n", // obs-fold in trailers
        b"0\r\nX: y\n",             // bare LF in trailers
    ] {
        assert!(
            dechunk(body, 1).is_err(),
            "{:?}",
            String::from_utf8_lossy(body)
        );
    }
}

#[test]
fn chunk_extensions_are_bounded() {
    let mut body = b"1;".to_vec();
    body.extend(std::iter::repeat_n(b'a', 5000));
    body.extend_from_slice(b"\r\nx\r\n0\r\n\r\n");
    assert_eq!(
        dechunk(&body, 64),
        Err(ChunkError("chunk extensions too long"))
    );
}

#[test]
fn the_largest_chunk_size_is_accepted_and_one_more_digit_is_not() {
    let mut d = ChunkedDecoder::new(Limits::default());
    let (used, chunk) = d.decode(b"ffffffffffffffff\r\nab").unwrap();
    assert_eq!(chunk, Chunk::Data(b"ab"));
    assert_eq!(used, 20);
}

// ---- encoding -------------------------------------------------------------

const DATE: &[u8; 29] = b"Thu, 12 Nov 2026 00:00:00 GMT";

#[test]
fn a_response_head_has_its_framing_date_and_fields() {
    let mut out = Vec::new();
    encode_response_head(
        &mut out,
        200,
        [
            (&b"content-type"[..], &b"text/plain"[..]),
            (b"Content-Length", b"999"),
            (b"transfer-encoding", b"x"),
        ],
        Framing::Length(13),
        DATE,
        true,
    )
    .unwrap();
    assert_eq!(
        String::from_utf8(out).unwrap(),
        "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: 13\r\ndate: Thu, 12 Nov 2026 00:00:00 GMT\r\n\r\n"
    );
}

#[test]
fn a_handlers_date_is_kept_and_close_is_said() {
    let mut out = Vec::new();
    encode_response_head(
        &mut out,
        599,
        [(&b"Date"[..], &b"x"[..])],
        Framing::Chunked,
        DATE,
        false,
    )
    .unwrap();
    assert_eq!(
        String::from_utf8(out).unwrap(),
        "HTTP/1.1 599 \r\nDate: x\r\ntransfer-encoding: chunked\r\nconnection: close\r\n\r\n"
    );
    let mut out = Vec::new();
    encode_response_head(&mut out, 204, [], Framing::NoBody, DATE, true).unwrap();
    assert!(
        String::from_utf8(out)
            .unwrap()
            .starts_with("HTTP/1.1 204 No Content\r\ndate:")
    );
}

#[test]
fn the_encoder_refuses_what_would_split_a_response() {
    let mut out = b"kept".to_vec();
    for (name, value, err) in [
        (
            &b"x"[..],
            &b"a\r\nset-cookie: evil"[..],
            EncodeError::InvalidFieldValue,
        ),
        (b"x\r\n", b"a", EncodeError::InvalidFieldName),
        (b"", b"a", EncodeError::InvalidFieldName),
        (b"x", b"a\0", EncodeError::InvalidFieldValue),
    ] {
        assert_eq!(
            encode_response_head(&mut out, 200, [(name, value)], Framing::NoBody, DATE, true),
            Err(err)
        );
        assert_eq!(out, b"kept", "nothing appended on error");
    }
    assert_eq!(
        encode_response_head(&mut out, 99, [], Framing::NoBody, DATE, true),
        Err(EncodeError::InvalidStatus)
    );
    assert_eq!(
        encode_last_chunk(&mut out, [(&b"x"[..], &b"\n"[..])]),
        Err(EncodeError::InvalidFieldValue)
    );
    assert_eq!(out, b"kept");
}

#[test]
fn chunks_round_trip_through_our_own_decoder() {
    let mut wire = Vec::new();
    for part in [&b"a"[..], &[b'x'; 300], &[b'y'; 16]] {
        wire.extend_from_slice(chunk_header(part.len()).as_bytes());
        wire.extend_from_slice(part);
        wire.extend_from_slice(CHUNK_END);
    }
    encode_last_chunk(&mut wire, [(&b"x-sum"[..], &b"42"[..])]).unwrap();
    assert!(wire.starts_with(b"1\r\na\r\n12c\r\n"));
    let (data, trailers, used) = dechunk(&wire, 5).unwrap();
    assert_eq!(data.len(), 317);
    assert_eq!(trailers, [("x-sum".into(), "42".into())]);
    assert_eq!(used, wire.len());
}

// ---- refusing early -------------------------------------------------------

#[test]
fn what_is_already_wrong_is_refused_before_the_head_ends() {
    for (partial, want) in [
        (&b"GET / HTTP/5.6"[..], RequestError::VersionNotSupported),
        (b"GET / HTTP/1.0\r", RequestError::VersionNotSupported),
        (
            b"GET / HTTX",
            RequestError::Malformed("invalid HTTP version"),
        ),
        (b"G(T", RequestError::Malformed("invalid method")),
        (
            b"GET /a\x01",
            RequestError::Malformed("invalid request-target"),
        ),
        (
            b"GET / HTTP/1.1\r\n Host: foo",
            RequestError::Malformed("obs-fold"),
        ),
        (
            b"GET / HTTP/1.1\r\nHo st",
            RequestError::Malformed("invalid field name"),
        ),
        (
            b"GET / HTTP/1.1\r\nX: a\x00",
            RequestError::Malformed("invalid field value"),
        ),
        (
            b"PUT / HTTP/1.1\r\nContent-Length: 1000000000000000000000\r\n",
            RequestError::Malformed("Content-Length overflows"),
        ),
        (
            b"PUT / HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 2\r\n",
            RequestError::Malformed("conflicting Content-Length"),
        ),
    ] {
        assert_eq!(
            parse(partial),
            Err(want),
            "{:?}",
            String::from_utf8_lossy(partial)
        );
    }
}

#[test]
fn every_prefix_of_several_good_heads_is_partial() {
    for raw in [
        &b"GET /a?b HTTP/1.1\r\nHost: h\r\nX:\r\nY: \t v \r\n\r\n"[..],
        b"OPTIONS * HTTP/1.1\r\nHost: h:1\r\nContent-Length: 0\r\n\r\n",
        b"CONNECT h:443 HTTP/1.1\r\nHost: h:443\r\n\r\n",
        b"\r\n\r\nPOST http://h/x HTTP/1.1\r\nHost: h\r\nTransfer-Encoding: gzip, chunked\r\n\r\n",
    ] {
        for n in 0..raw.len() {
            assert_eq!(
                parse(&raw[..n]),
                Ok(Parsed::Partial),
                "{:?} at {n}",
                String::from_utf8_lossy(raw)
            );
        }
    }
}

// ---- chunk extensions -----------------------------------------------------

#[test]
fn chunk_extensions_follow_their_grammar() {
    for ext in [
        &b";a"[..],
        b";a=b",
        b";a=b;c",
        b"; a = b ; c",
        b" ;a",
        b"\t;a=\"q \\\"x\\\" y\"",
        b";a=\"\"",
        b";a=\"x\" ;b",
    ] {
        let mut body = b"1".to_vec();
        body.extend_from_slice(ext);
        body.extend_from_slice(b"\r\nx\r\n0\r\n\r\n");
        let (data, _, _) = dechunk(&body, 1).unwrap_or_else(|e| panic!("{ext:?}: {e}"));
        assert_eq!(data, b"x", "{:?}", String::from_utf8_lossy(ext));
    }
    for ext in [
        &b" junk"[..],
        b";",
        b"; ",
        b";=b",
        b";a=",
        b";a=\"open",
        b";a=b c",
        b";a b",
        b";a=\"x\x01\"",
        b";a,b",
        // Whitespace is only allowed before a `;` (or around `=`): never at
        // the end of the line.
        b" ",
        b";a ",
        b";a=b ",
        b";a=\"x\" ",
    ] {
        let mut body = b"1".to_vec();
        body.extend_from_slice(ext);
        body.extend_from_slice(b"\r\nx\r\n0\r\n\r\n");
        assert!(
            dechunk(&body, 1).is_err(),
            "{:?}",
            String::from_utf8_lossy(ext)
        );
    }
}

// ---- the client's half: responses and requests ----------------------------

fn response(raw: &[u8], to_head: bool) -> Result<ResponseHead, ResponseError> {
    let mut fields = Vec::new();
    match parse_response(raw, &Limits::default(), &mut fields, to_head)? {
        ParsedResponse::Complete(head) => Ok(head),
        ParsedResponse::Partial => panic!("partial: {:?}", String::from_utf8_lossy(raw)),
    }
}

#[test]
fn response_framing_follows_rfc_9112_section_6_3() {
    let body = |raw: &[u8], head| response(raw, head).unwrap().body;
    assert_eq!(
        body(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n", false),
        ResponseBody::Length(5)
    );
    // A HEAD response, 1xx, 204 and 304 have no body whatever they say.
    assert_eq!(
        body(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n", true),
        ResponseBody::Empty
    );
    for s in ["100 Continue", "204 No Content", "304 Not Modified"] {
        let raw = format!("HTTP/1.1 {s}\r\nContent-Length: 5\r\n\r\n");
        assert_eq!(body(raw.as_bytes(), false), ResponseBody::Empty, "{s}");
    }
    assert_eq!(
        body(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip, chunked\r\n\r\n",
            false
        ),
        ResponseBody::Chunked
    );
    // No length: the body ends when the connection does, which is never reused.
    let open = response(b"HTTP/1.1 200 OK\r\n\r\n", false).unwrap();
    assert_eq!(
        (open.body, open.keep_alive),
        (ResponseBody::UntilClose, false)
    );
    let gzip = response(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip\r\n\r\n", false).unwrap();
    assert_eq!(
        (gzip.body, gzip.keep_alive),
        (ResponseBody::UntilClose, false)
    );
    // Both framings: the coding wins and the connection is not reused.
    let both = response(
        b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\nTransfer-Encoding: chunked\r\n\r\n",
        false,
    )
    .unwrap();
    assert_eq!((both.body, both.keep_alive), (ResponseBody::Chunked, false));
}

#[test]
fn status_lines_are_http_1_1_with_a_three_digit_code() {
    let head = response(
        b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n",
        false,
    )
    .unwrap();
    assert_eq!(head.status, 404);
    // A server that sends no reason phrase often drops its space too.
    assert_eq!(
        response(b"HTTP/1.1 200\r\nContent-Length: 0\r\n\r\n", false)
            .unwrap()
            .status,
        200
    );
    assert!(
        response(b"HTTP/1.1 101 Switching Protocols\r\n\r\n", false).is_ok_and(|h| !h.is_interim())
    );
    assert!(response(b"HTTP/1.1 103 Early Hints\r\n\r\n", false).is_ok_and(|h| h.is_interim()));
    assert_eq!(
        response(b"HTTP/1.0 200 OK\r\n\r\n", false),
        Err(ResponseError::VersionNotSupported)
    );
    for raw in [
        &b"HTTP/1.1 20 OK\r\n\r\n"[..],
        b"HTTP/1.1 2000 OK\r\n\r\n",
        b"HTTP/1.1 099 x\r\n\r\n",
        b"HTTP/1.1 200 O\x01K\r\n\r\n",
        b"HTTP/1.1 200 OK\r\nX: a\r\n b\r\n\r\n",
        b"HTTP/1.1 200 OK\nX: a\r\n\r\n",
        b"HTTP/1.1 200 OK\r\nContent-Length: 1\r\nContent-Length: 2\r\n\r\n",
    ] {
        assert!(
            matches!(response(raw, false), Err(ResponseError::Malformed(_))),
            "{:?}",
            String::from_utf8_lossy(raw)
        );
    }
}

#[test]
fn every_prefix_of_a_good_response_head_is_partial() {
    let raw = b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nTransfer-Encoding: chunked\r\n\r\n";
    let mut fields = Vec::new();
    for n in 0..raw.len() {
        assert_eq!(
            parse_response(&raw[..n], &Limits::default(), &mut fields, false),
            Ok(ParsedResponse::Partial),
            "prefix {n}"
        );
    }
}

#[test]
fn a_request_head_is_encoded_with_its_host_and_framing() {
    let mut out = Vec::new();
    encode_request_head(
        &mut out,
        b"POST",
        b"/upload?x=1",
        b"example.test:8080",
        [
            (&b"content-type"[..], &b"text/plain"[..]),
            (b"Host", b"evil"),
            (b"content-length", b"9"),
        ],
        Framing::Length(4),
    )
    .unwrap();
    assert_eq!(
        String::from_utf8(out.clone()).unwrap(),
        "POST /upload?x=1 HTTP/1.1\r\nhost: example.test:8080\r\ncontent-type: text/plain\r\ncontent-length: 4\r\n\r\n"
    );
    // What we write, our own server parses back the same way.
    let mut fields = Vec::new();
    let Ok(Parsed::Complete(head)) = parse_request(&out, &Limits::default(), &mut fields) else {
        panic!("our own request did not parse");
    };
    assert_eq!(head.body, Body::Length(4));
    assert_eq!(head.host.of(&out), b"example.test:8080");
}

#[test]
fn the_request_encoder_refuses_what_would_split_a_request() {
    let mut out = b"kept".to_vec();
    for (method, target, host, value, err) in [
        (
            &b"G T"[..],
            &b"/"[..],
            &b"h"[..],
            &b"v"[..],
            EncodeError::InvalidMethod,
        ),
        (b"GET", b"/a b", b"h", b"v", EncodeError::InvalidTarget),
        (b"GET", b"/\r\nX: y", b"h", b"v", EncodeError::InvalidTarget),
        (
            b"GET",
            b"/",
            b"h\r\nx",
            b"v",
            EncodeError::InvalidFieldValue,
        ),
        (
            b"GET",
            b"/",
            b"h",
            b"a\r\nb",
            EncodeError::InvalidFieldValue,
        ),
    ] {
        assert_eq!(
            encode_request_head(
                &mut out,
                method,
                target,
                host,
                [(&b"x"[..], value)],
                Framing::NoBody
            ),
            Err(err)
        );
        assert_eq!(out, b"kept");
    }
}
