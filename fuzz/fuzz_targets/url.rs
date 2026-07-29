//! URL parsing and component read-back.
//!
//! The `url` crate is fuzzed upstream; what is ours is the UTF-16 offset
//! remapping the JS side slices components out of, and the setter path where a
//! malformed value is specified to be a silent no-op.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: (&str, Option<&str>, &str, &str)| {
    let (input, base, component, value) = data;
    es_runtime::fuzz::url_components(input, base);
    es_runtime::fuzz::url_set(input, component, value);
});
