//! Decompression of arbitrary bytes, chunked the way a `DecompressionStream`
//! feeds it. Corrupt input, trailing junk and truncation must be errors.
#![no_main]

use libfuzzer_sys::fuzz_target;

const FORMATS: [&str; 4] = ["gzip", "deflate", "deflate-raw", "brotli"];

fuzz_target!(|data: (u8, Vec<&[u8]>)| {
    let (which, chunks) = data;
    let format = FORMATS[which as usize % FORMATS.len()];
    es_runtime::fuzz::decompress(format, &chunks);
});
