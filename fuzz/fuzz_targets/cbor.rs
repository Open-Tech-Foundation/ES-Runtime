//! Decodes CBOR over arbitrary bytes. The reader and the canonical writer both
//! recurse over guest input and both do length arithmetic on untrusted
//! headers, so the cases that matter are truncation, oversized lengths and
//! nesting beyond the depth bound.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: &[u8]| {
    es_runtime::fuzz::decode_cbor(input);
});
