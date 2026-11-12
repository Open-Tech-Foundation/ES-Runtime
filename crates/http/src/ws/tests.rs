//! RFC 6455 framing: each rule a peer can break, with the close code it is
//! answered with, and round trips through the encoder in both roles.

use super::*;

const MASK: [u8; 4] = [0x37, 0xfa, 0x21, 0x3d];

/// A frame as a client sends it (masked), for a server decoder.
fn client_frame(opcode: Opcode, fin: bool, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    encode_frame(&mut out, opcode, fin, payload, Some(MASK));
    out
}

fn server() -> Decoder {
    Decoder::new(Role::Server, Limits::default())
}

/// Decodes every frame in `bytes`, collecting events, or the first error.
fn decode_all(d: &mut Decoder, bytes: &[u8]) -> Result<Vec<Event>, WsError> {
    let mut at = 0;
    let mut events = Vec::new();
    while let Some((event, used)) = d.decode(&bytes[at..])? {
        at += used;
        events.extend(event);
    }
    assert_eq!(at, bytes.len(), "bytes left over");
    Ok(events)
}

fn code_of(result: Result<Vec<Event>, WsError>) -> u16 {
    result.expect_err("should have been refused").code
}

#[test]
fn rfc_6455_section_5_7_examples_decode() {
    // §5.7: a single-frame unmasked text "Hello", and the masked one.
    let mut client = Decoder::new(Role::Client, Limits::default());
    let unmasked = [0x81, 0x05, 0x48, 0x65, 0x6c, 0x6c, 0x6f];
    assert_eq!(
        decode_all(&mut client, &unmasked).unwrap(),
        [Event::Text("Hello".into())]
    );
    let masked = [
        0x81, 0x85, 0x37, 0xfa, 0x21, 0x3d, 0x7f, 0x9f, 0x4d, 0x51, 0x58,
    ];
    assert_eq!(
        decode_all(&mut server(), &masked).unwrap(),
        [Event::Text("Hello".into())]
    );
    // The encoder writes exactly those bytes.
    assert_eq!(client_frame(Opcode::Text, true, b"Hello"), masked);
    let mut out = Vec::new();
    encode_frame(&mut out, Opcode::Text, true, b"Hello", None);
    assert_eq!(out, unmasked);
    // A fragmented unmasked text message: "Hel" then "lo".
    let fragments = [0x01, 0x03, 0x48, 0x65, 0x6c, 0x80, 0x02, 0x6c, 0x6f];
    assert_eq!(
        decode_all(&mut client, &fragments).unwrap(),
        [Event::Text("Hello".into())]
    );
    // Unmasked ping, and a 256-byte and a 64 KiB binary message.
    assert_eq!(
        decode_all(&mut client, &[0x89, 0x05, 0x48, 0x65, 0x6c, 0x6c, 0x6f]).unwrap(),
        [Event::Ping(b"Hello".to_vec())]
    );
    let mut long = vec![0x82, 0x7E, 0x01, 0x00];
    long.extend(vec![0u8; 256]);
    assert_eq!(
        decode_all(&mut client, &long).unwrap(),
        [Event::Binary(vec![0; 256])]
    );
    let mut longer = vec![0x82, 0x7F, 0, 0, 0, 0, 0, 1, 0, 0];
    longer.extend(vec![0u8; 65536]);
    assert_eq!(
        decode_all(&mut client, &longer).unwrap(),
        [Event::Binary(vec![0; 65536])]
    );
}

#[test]
fn a_frame_arriving_a_byte_at_a_time_waits_for_the_rest() {
    let frame = client_frame(Opcode::Binary, true, &[9; 300]);
    let mut d = server();
    for end in 0..frame.len() {
        assert_eq!(d.decode(&frame[..end]).unwrap(), None, "decoded at {end}");
    }
    assert_eq!(
        d.decode(&frame).unwrap(),
        Some((Some(Event::Binary(vec![9; 300])), frame.len()))
    );
}

#[test]
fn control_frames_may_come_between_fragments() {
    let mut bytes = client_frame(Opcode::Text, false, b"frag");
    bytes.extend(client_frame(Opcode::Ping, true, b"p"));
    bytes.extend(client_frame(Opcode::Continuation, false, b"men"));
    bytes.extend(client_frame(Opcode::Pong, true, b""));
    bytes.extend(client_frame(Opcode::Continuation, true, b"ted"));
    assert_eq!(
        decode_all(&mut server(), &bytes).unwrap(),
        [
            Event::Ping(b"p".to_vec()),
            Event::Pong(Vec::new()),
            Event::Text("fragmented".into())
        ]
    );
}

#[test]
fn masking_must_match_the_direction() {
    let mut unmasked = Vec::new();
    encode_frame(&mut unmasked, Opcode::Text, true, b"x", None);
    assert_eq!(
        code_of(decode_all(&mut server(), &unmasked)),
        PROTOCOL_ERROR
    );
    let mut client = Decoder::new(Role::Client, Limits::default());
    assert_eq!(
        code_of(decode_all(
            &mut client,
            &client_frame(Opcode::Text, true, b"x")
        )),
        PROTOCOL_ERROR
    );
}

#[test]
fn each_protocol_violation_is_a_1002() {
    let reserved_bit = {
        let mut f = client_frame(Opcode::Text, true, b"x");
        f[0] |= 0x40;
        f
    };
    let reserved_opcode = {
        let mut f = client_frame(Opcode::Binary, true, b"x");
        f[0] = (f[0] & 0xF0) | 0x3;
        f
    };
    let fragmented_ping = client_frame(Opcode::Ping, false, b"x");
    let long_ping = client_frame(Opcode::Ping, true, &[0; 126]);
    let orphan_continuation = client_frame(Opcode::Continuation, true, b"x");
    let mut interleaved = client_frame(Opcode::Text, false, b"a");
    interleaved.extend(client_frame(Opcode::Text, true, b"b"));
    let one_byte_close = client_frame(Opcode::Close, true, &[3]);
    // A 5-byte payload with a 16-bit length: not the shortest form.
    let long_form = [0x82, 0xFE, 0x00, 0x05, 0, 0, 0, 0, 1, 2, 3, 4, 5];
    let cases: [(&str, &[u8]); 8] = [
        ("reserved bit", &reserved_bit),
        ("reserved opcode", &reserved_opcode),
        ("fragmented ping", &fragmented_ping),
        ("126-byte ping", &long_ping),
        ("orphan continuation", &orphan_continuation),
        ("message inside a message", &interleaved),
        ("one-byte close", &one_byte_close),
        ("non-minimal length", &long_form),
    ];
    for (name, bytes) in cases {
        assert_eq!(
            code_of(decode_all(&mut server(), bytes)),
            PROTOCOL_ERROR,
            "{name}"
        );
    }
}

#[test]
fn close_codes_on_the_wire_are_checked() {
    for code in [
        1000u16, 1001, 1002, 1003, 1007, 1008, 1009, 1010, 1011, 1012, 1013, 1014, 3000, 3999,
        4000, 4999,
    ] {
        let frame = client_frame(Opcode::Close, true, &code.to_be_bytes());
        assert_eq!(
            decode_all(&mut server(), &frame).unwrap(),
            [Event::Close {
                code: Some(code),
                reason: String::new()
            }],
            "{code}"
        );
    }
    // Autobahn 7.9.x's invalid ones, and the report-only 1005/1006/1015.
    for code in [
        0u16, 999, 1004, 1005, 1006, 1015, 1016, 1100, 2000, 2999, 5000, 65535,
    ] {
        let frame = client_frame(Opcode::Close, true, &code.to_be_bytes());
        assert_eq!(
            code_of(decode_all(&mut server(), &frame)),
            PROTOCOL_ERROR,
            "{code}"
        );
    }
    assert_eq!(
        decode_all(&mut server(), &client_frame(Opcode::Close, true, b"")).unwrap(),
        [Event::Close {
            code: None,
            reason: String::new()
        }]
    );
    let mut bad_reason = 1000u16.to_be_bytes().to_vec();
    bad_reason.extend([0xCE, 0xBA, 0xE1, 0xBD]); // truncated UTF-8
    assert_eq!(
        code_of(decode_all(
            &mut server(),
            &client_frame(Opcode::Close, true, &bad_reason)
        )),
        INVALID_PAYLOAD
    );
}

#[test]
fn invalid_utf8_fails_as_soon_as_it_is_certain() {
    // 0xF4 0x90 can never begin a valid sequence (above U+10FFFF): refused
    // at the first fragment, before the message ends.
    let mut d = server();
    let first = client_frame(Opcode::Text, false, &[b'o', b'k', 0xF4, 0x90]);
    assert_eq!(d.decode(&first).unwrap_err().code, INVALID_PAYLOAD);
    // A sequence split across fragments is fine.
    let euro = "€".as_bytes(); // E2 82 AC
    let mut bytes = client_frame(Opcode::Text, false, &euro[..1]);
    bytes.extend(client_frame(Opcode::Continuation, false, &euro[1..2]));
    bytes.extend(client_frame(Opcode::Continuation, true, &euro[2..]));
    assert_eq!(
        decode_all(&mut server(), &bytes).unwrap(),
        [Event::Text("€".into())]
    );
    // But one left incomplete when the message ends is not.
    let cut = client_frame(Opcode::Text, true, &euro[..2]);
    assert_eq!(code_of(decode_all(&mut server(), &cut)), INVALID_PAYLOAD);
    // Binary is not checked.
    assert_eq!(
        decode_all(&mut server(), &client_frame(Opcode::Binary, true, &[0xFF])).unwrap(),
        [Event::Binary(vec![0xFF])]
    );
}

#[test]
fn limits_refuse_at_the_header_with_1009() {
    let limits = Limits {
        max_frame: 1000,
        max_message: 1500,
    };
    let mut d = Decoder::new(Role::Server, limits);
    let big = client_frame(Opcode::Binary, true, &[0; 1001]);
    // Refused from the header alone, before the payload is buffered.
    assert_eq!(d.decode(&big[..8]).unwrap_err().code, TOO_BIG);
    let mut d = Decoder::new(Role::Server, limits);
    let mut bytes = client_frame(Opcode::Binary, false, &[0; 1000]);
    bytes.extend(client_frame(Opcode::Continuation, true, &[0; 501]));
    assert_eq!(code_of(decode_all(&mut d, &bytes)), TOO_BIG);
}

#[test]
fn close_reasons_are_cut_at_a_character_boundary() {
    let reason = "é".repeat(100); // 200 bytes
    let mut out = Vec::new();
    encode_close(&mut out, Some(1000), &reason, None);
    let mut client = Decoder::new(Role::Client, Limits::default());
    let events = decode_all(&mut client, &out).unwrap();
    let [
        Event::Close {
            code: Some(1000),
            reason: got,
        },
    ] = events.as_slice()
    else {
        panic!("{events:?}");
    };
    assert_eq!(got.len(), 122, "123 bytes would split a character");
    assert!(reason.starts_with(got.as_str()));
}

#[test]
fn masking_round_trips_at_every_length() {
    for len in 0..70 {
        let data: Vec<u8> = (0..len as u8).collect();
        let mut masked = data.clone();
        apply_mask(&mut masked, MASK);
        if len > 0 {
            assert_ne!(masked, data);
        }
        apply_mask(&mut masked, MASK);
        assert_eq!(masked, data);
    }
}

#[test]
fn the_opening_handshake_is_checked_as_rfc_6455_4_2_1_says() {
    // §1.3's example key.
    let ok: &[(&[u8], &[u8])] = &[
        (b"upgrade", b"websocket"),
        (b"connection", b"keep-alive, Upgrade"),
        (b"sec-websocket-version", b"13"),
        (b"sec-websocket-key", b"dGhlIHNhbXBsZSBub25jZQ=="),
    ];
    let lookup = |fields: &'static [(&'static [u8], &'static [u8])]| {
        move |name: &[u8]| fields.iter().find(|(n, _)| *n == name).map(|(_, v)| *v)
    };
    let fields: &'static [(&[u8], &[u8])] = Box::leak(ok.to_vec().into_boxed_slice());
    assert_eq!(
        check_upgrade(b"GET", lookup(fields)),
        Ok(&b"dGhlIHNhbXBsZSBub25jZQ=="[..])
    );
    assert_eq!(
        check_upgrade(b"POST", lookup(fields)),
        Err(UpgradeRefusal::NotWebSocket)
    );
    let with =
        |name: &'static [u8], value: &'static [u8]| -> &'static [(&'static [u8], &'static [u8])] {
            let mut v = ok.to_vec();
            v.retain(|(n, _)| *n != name);
            if !value.is_empty() {
                v.push((name, value));
            }
            Box::leak(v.into_boxed_slice())
        };
    assert_eq!(
        check_upgrade(b"GET", lookup(with(b"upgrade", b"h2c"))),
        Err(UpgradeRefusal::NotWebSocket)
    );
    assert_eq!(
        check_upgrade(b"GET", lookup(with(b"connection", b"close"))),
        Err(UpgradeRefusal::NotWebSocket)
    );
    assert_eq!(
        check_upgrade(b"GET", lookup(with(b"sec-websocket-version", b"8"))),
        Err(UpgradeRefusal::Version)
    );
    assert_eq!(
        check_upgrade(b"GET", lookup(with(b"sec-websocket-version", b""))),
        Err(UpgradeRefusal::Version)
    );
    for key in [
        &b""[..],
        b"short==",
        b"dGhlIHNhbXBsZSBub25jZQ",
        b"dGhlIHNhbXBsZSBub25jZR==",
        b"dGhlIHNhbXBsZSBub25j!Q==",
    ] {
        let fields = with(
            b"sec-websocket-key",
            Box::leak(key.to_vec().into_boxed_slice()),
        );
        assert_eq!(
            check_upgrade(b"GET", lookup(fields)),
            Err(UpgradeRefusal::BadKey),
            "{:?}",
            String::from_utf8_lossy(key)
        );
    }
}
