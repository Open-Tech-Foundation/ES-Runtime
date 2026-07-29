//! The hand-written RFC 8410 DER parsers for Ed25519 / X25519 keys, and
//! `atob`. Both walk untrusted bytes by index, which is where a short or odd
//! input turns into a panic.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: (&[u8], &str)| {
    let (der, text) = data;
    es_runtime::fuzz::curve25519_der("Ed25519", der);
    es_runtime::fuzz::curve25519_der("X25519", der);
    es_runtime::fuzz::atob(text);
});
