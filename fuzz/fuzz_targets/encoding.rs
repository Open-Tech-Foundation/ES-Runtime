//! `TextDecoder` over arbitrary labels and bytes, lossy and fatal, one-shot and
//! streamed across a chunk boundary.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: (&str, &[u8], bool, bool)| {
    let (label, bytes, fatal, ignore_bom) = data;
    es_runtime::fuzz::decode(label, bytes, fatal, ignore_bom);
});
