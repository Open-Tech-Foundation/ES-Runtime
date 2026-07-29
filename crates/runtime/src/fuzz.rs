//! Entry points for the fuzz targets under `fuzz/` (SPEC §5).
//!
//! Compiled only with the `fuzzing` feature, which nothing but the fuzz crate
//! enables. It is **not** public API: it exists so a fuzz target can reach the
//! parsers and decoders that ordinarily sit behind the op boundary, without
//! either making them `pub` for everyone or standing up a V8 isolate per
//! iteration (which would cut the iteration rate by orders of magnitude and
//! fuzz V8 rather than this code).
//!
//! What is worth fuzzing here is *our* handling of untrusted bytes: index
//! arithmetic, hand-written framing, and the boundaries around vetted crates —
//! not the vetted crates themselves, which are fuzzed upstream.

/// Parses a URL and reads back every component, exercising the UTF-16 offset
/// remapping the JS side slices with. Wrong offsets there are how a component
/// accessor returns bytes from the middle of a code point.
pub fn url_components(input: &str, base: Option<&str>) {
    crate::url_ops::fuzz_parse(input, base);
}

/// Applies a component setter to an href — the path where a malformed value is
/// specified to be a silent no-op rather than an error.
pub fn url_set(href: &str, component: &str, value: &str) {
    crate::url_ops::fuzz_set(href, component, value);
}

/// Compiles a URLPattern component and reads back its emitted regex source.
pub fn urlpattern(pattern: &str) {
    crate::urlpattern_ops::fuzz_component(pattern);
}

/// Decodes arbitrary bytes under an arbitrary label, in both lossy and fatal
/// modes. The label table and the decoders are `encoding_rs`'; the capacity
/// arithmetic around them is ours.
pub fn decode(label: &str, bytes: &[u8], fatal: bool, ignore_bom: bool) {
    crate::encoding_ops::fuzz_decode(label, bytes, fatal, ignore_bom);
}

/// Feeds arbitrary bytes to a decompressor, in chunks, then finishes it — the
/// shape a `DecompressionStream` sees. Corrupt input, trailing junk and
/// truncation must all come back as errors rather than panics or silent output.
pub fn decompress(format: &str, chunks: &[&[u8]]) {
    crate::compression_ops::fuzz_decompress(format, chunks);
}

/// Parses XML into the runtime's value tree — recursive descent over untrusted
/// input, so depth and malformed nesting are the interesting cases.
pub fn parse_xml(input: &str) {
    crate::serialization_ops::fuzz_parse_xml(input);
}

/// Decodes base64 the way `atob` does, including its stricter-than-usual
/// whitespace and padding rules.
pub fn atob(input: &str) {
    crate::base64_ops::fuzz_decode(input);
}

/// Imports Curve25519 keys from DER. This parser is hand-written against a
/// fixed byte layout, which makes it exactly the kind of code worth fuzzing:
/// every slice index in it is a potential panic on a short or odd input.
pub fn curve25519_der(curve: &str, der: &[u8]) {
    crate::curve25519_ops::fuzz_import(curve, der);
}
