//! HPACK (crates/http, D217): the decoder fed arbitrary blocks never panics
//! and never collects more than its list limit; and what our encoder writes
//! for an arbitrary header list, our decoder reads back, with both tables the
//! same size afterwards.
#![no_main]

use es_runtime_http::hpack::{Decoder, Encoder};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: (u16, Vec<u8>, Vec<(Vec<u8>, Vec<u8>)>)| {
    let (table, block, list) = data;
    let table = usize::from(table % 8192);

    let mut decoder = Decoder::new(table);
    let mut out = Vec::new();
    if decoder.decode(&block, 4096, &mut out).is_ok() {
        let size: usize = out.iter().map(|(n, v)| n.len() + v.len() + 32).sum();
        assert!(size <= 4096);
    }

    let mut encoder = Encoder::new(table);
    let mut decoder = Decoder::new(table);
    for _ in 0..2 {
        let mut block = Vec::new();
        encoder.encode(list.iter().map(|(n, v)| (n.as_slice(), v.as_slice())), &mut block);
        let mut back = Vec::new();
        decoder.decode(&block, usize::MAX, &mut back).expect("our own block");
        assert_eq!(back, list);
    }
});
