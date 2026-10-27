//! Validates and walks BSON over arbitrary bytes, and parses them as Decimal128
//! text. Document lengths, string lengths and terminators are untrusted, nesting
//! is bounded, and a Decimal128 exponent can be arbitrarily long.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: &[u8]| {
    es_runtime::fuzz::decode_bson(input);
});
