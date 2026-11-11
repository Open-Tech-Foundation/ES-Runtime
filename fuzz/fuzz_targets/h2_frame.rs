//! HTTP/2 frames (crates/http, D217): reading arbitrary bytes never panics and
//! never claims more than it was given; and a field block of any size, split
//! into HEADERS and CONTINUATION at any frame size, reads back whole.
#![no_main]

use es_runtime_http::h2::frame::{
    DEFAULT_MAX_FRAME_SIZE, Frame, MAX_MAX_FRAME_SIZE, encode_headers, parse_frame,
};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: (u32, &[u8], Vec<u8>)| {
    let (size, wire, block) = data;
    let max = DEFAULT_MAX_FRAME_SIZE + size % (MAX_MAX_FRAME_SIZE - DEFAULT_MAX_FRAME_SIZE + 1);

    let mut at = 0;
    while let Ok(Some((_, used))) = parse_frame(&wire[at..], max) {
        assert!(used >= 9 && at + used <= wire.len());
        at += used;
    }

    let mut out = Vec::new();
    encode_headers(&mut out, 1, &block, false, DEFAULT_MAX_FRAME_SIZE);
    let mut at = 0;
    let mut joined = Vec::new();
    let mut ended = false;
    while at < out.len() {
        let (frame, used) = parse_frame(&out[at..], DEFAULT_MAX_FRAME_SIZE).unwrap().unwrap();
        at += used;
        match frame {
            Frame::Headers { block, end_headers, .. } | Frame::Continuation { block, end_headers, .. } => {
                assert!(!ended);
                joined.extend_from_slice(block);
                ended = end_headers;
            }
            other => panic!("{other:?}"),
        }
    }
    assert!(ended);
    assert_eq!(joined, block);
});
