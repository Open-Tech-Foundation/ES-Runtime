//! `runtime:pdf` over untrusted bytes: open (with repair), every read the
//! module offers, then a save with page, bookmark and sanitize edits. The
//! decode limit is small, so a decompression bomb is refused early.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    es_runtime::fuzz::pdf(data);
});
