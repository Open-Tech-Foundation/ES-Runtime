//! `runtime:images` over untrusted bytes: whichever decoder the leading bytes
//! select (PNG, JPEG, WebP, GIF, TIFF, AVIF, JPEG XL), then metadata, stats
//! and a re-encode. The pixel limit is small, so a header claiming a huge
//! canvas is refused rather than allocated.
#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    es_runtime::fuzz::image(data);
});
