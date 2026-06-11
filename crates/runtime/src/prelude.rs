//! Assembly of the JS prelude (DECISIONS.md D8).
//!
//! Pure-JS web APIs ship as a prelude. D8 bakes it into a V8 startup snapshot;
//! for Phase 4 it is evaluated at runtime construction instead — identical
//! behavior, just slower context creation — with snapshot-baking landing in
//! Phase 8 (SPEC.md §6.8). Each fragment is a self-contained IIFE that installs
//! its globals on `globalThis`, using the host ops under `globalThis.__ops`.

/// The concatenated prelude source, in load order. `console` first so later
/// fragments (e.g. `globals`' `reportError`) can route through it.
pub(crate) fn source() -> String {
    [
        // DOMException first: later fragments throw it.
        include_str!("prelude/dom-exception.js"),
        // console before globals: reportError routes through it.
        include_str!("prelude/console.js"),
        include_str!("prelude/performance.js"),
        include_str!("prelude/globals.js"),
        include_str!("prelude/encoding.js"),
        include_str!("prelude/base64.js"),
        include_str!("prelude/structured-clone.js"),
        include_str!("prelude/url.js"),
        // events before abort: AbortSignal extends EventTarget.
        include_str!("prelude/events.js"),
        include_str!("prelude/abort.js"),
        include_str!("prelude/streams.js"),
        // encoding streams need TransformStream + TextEncoder/TextDecoder.
        include_str!("prelude/encoding-streams.js"),
        // fetch family: blob before fetch (fetch bodies may be Blob/FormData).
        include_str!("prelude/blob.js"),
        include_str!("prelude/fetch.js"),
    ]
    .join("\n")
}
