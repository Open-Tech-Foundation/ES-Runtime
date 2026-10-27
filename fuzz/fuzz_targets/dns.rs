//! Reads arbitrary bytes as a DNS response. Every count and length in a
//! response is untrusted, names compress through pointers that could loop or
//! point past the end, and each record type has its own layout.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: &[u8]| {
    es_runtime_default_providers::fuzz_dns_response(input);
});
