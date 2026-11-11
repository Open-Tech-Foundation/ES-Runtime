//! An HTTP/2 server connection (crates/http, D217) fed what a hostile client
//! could send: a valid preface, then arbitrary bytes in arbitrary pieces.
//! Beyond not panicking: it never claims more input than it was given, what it
//! owes the peer stays under the flood cap, and once it has ended the
//! connection it reads nothing more. The application side answers every
//! request it sees, so the response paths run too.
#![no_main]

use es_runtime_http::h2::conn::{Config, Connection, Event, MAX_PENDING_OUTPUT};
use es_runtime_http::h2::frame::{PREFACE, encode_settings};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: (Vec<u8>, &[u8])| {
    let (cuts, wire) = data;
    let mut server = Connection::server(Config {
        max_concurrent_streams: 8,
        max_pending_resets: 4,
        ..Config::default()
    });
    let mut hello = PREFACE.to_vec();
    encode_settings(&mut hello, &[]);
    server.receive(&hello).unwrap();

    let mut pending: Vec<u8> = Vec::new();
    let mut at = 0;
    let mut cut = cuts.iter().cycle();
    while at < wire.len() {
        let n = usize::from(*cut.next().unwrap_or(&255)).max(1).min(wire.len() - at);
        pending.extend_from_slice(&wire[at..at + n]);
        at += n;
        match server.receive(&pending) {
            Ok(used) => {
                assert!(used <= pending.len());
                pending.drain(..used);
            }
            Err(_) => {
                assert!(server.is_failed());
                assert!(server.receive(b"more").is_err());
                return;
            }
        }
        assert!(server.output().len() <= MAX_PENDING_OUTPUT + 64 * 1024);
        while let Some(event) = server.poll_event() {
            match event {
                Event::Headers { stream, end_stream, .. } => {
                    let _ = server.send_headers(stream, [(&b":status"[..], &b"200"[..])], false);
                    let _ = server.send_data(stream, b"ok", end_stream);
                }
                Event::Data { stream, data, .. } => server.release(stream, data.len()),
                Event::Reset { stream, .. } => server.done_with(stream),
                _ => {}
            }
        }
        let n = server.output().len();
        server.consume_output(n);
    }
});
