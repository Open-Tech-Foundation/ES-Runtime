//! WebSocket framing (crates/http, D217) fed arbitrary bytes, as a server
//! decoder (what a hostile client sends) and as a client decoder. Beyond not
//! panicking: no decode claims more than it was given, every text event is
//! UTF-8, a refusal carries one of the three codes framing can fail with, and
//! whatever is accepted re-encodes and decodes to the same event.
#![no_main]

use es_runtime_http::ws::{
    Decoder, Event, INVALID_PAYLOAD, Limits, Opcode, PROTOCOL_ERROR, Role, TOO_BIG, encode_close,
    encode_frame,
};
use libfuzzer_sys::fuzz_target;

fn check(role: Role, data: &[u8]) {
    let limits = Limits { max_frame: 1 << 16, max_message: 1 << 18 };
    let mut d = Decoder::new(role, limits);
    let mut at = 0;
    loop {
        match d.decode(&data[at..]) {
            Ok(None) => return,
            Ok(Some((event, used))) => {
                assert!(used > 0 && used <= data.len() - at);
                at += used;
                if let Some(event) = event {
                    round_trip(role, &event, limits);
                }
            }
            Err(e) => {
                assert!([PROTOCOL_ERROR, INVALID_PAYLOAD, TOO_BIG].contains(&e.code));
                return;
            }
        }
    }
}

/// Encodes `event` as the peer of `role` would, and decodes it again.
fn round_trip(role: Role, event: &Event, limits: Limits) {
    let mask = (role == Role::Server).then_some([1, 2, 3, 4]);
    let mut out = Vec::new();
    match event {
        Event::Text(t) => encode_frame(&mut out, Opcode::Text, true, t.as_bytes(), mask),
        Event::Binary(b) => encode_frame(&mut out, Opcode::Binary, true, b, mask),
        Event::Ping(p) => encode_frame(&mut out, Opcode::Ping, true, p, mask),
        Event::Pong(p) => encode_frame(&mut out, Opcode::Pong, true, p, mask),
        Event::Close { code, reason } => encode_close(&mut out, *code, reason, mask),
    }
    let mut d = Decoder::new(role, limits);
    let (again, used) = d.decode(&out).expect("our own frame").expect("whole");
    assert_eq!(used, out.len());
    assert_eq!(again.as_ref(), Some(event));
}

fuzz_target!(|data: &[u8]| {
    check(Role::Server, data);
    check(Role::Client, data);
});
