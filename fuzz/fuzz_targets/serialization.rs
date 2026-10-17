//! Text parsing into the runtime's value tree over untrusted input: XML
//! (recursive descent, where depth and malformed nesting are the interesting
//! cases) and CSV (a hand-written byte-oriented scanner, where quote and length
//! boundaries are).
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: &str| {
    es_runtime::fuzz::parse_xml(input);
    es_runtime::fuzz::parse_csv(input);
});
