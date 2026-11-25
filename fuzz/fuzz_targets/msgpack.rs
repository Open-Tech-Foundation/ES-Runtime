//! Decodes MessagePack over arbitrary bytes: the reader and writer are
//! hand-written over untrusted lengths and nesting, and the fast path's
//! validator must agree with the reader about what is a document.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: &[u8]| {
    es_runtime::fuzz::decode_msgpack(input);
});
