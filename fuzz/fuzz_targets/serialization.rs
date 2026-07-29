//! XML parsing into the runtime's value tree — recursive descent over untrusted
//! input, where depth and malformed nesting are the interesting cases.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|input: &str| {
    es_runtime::fuzz::parse_xml(input);
});
