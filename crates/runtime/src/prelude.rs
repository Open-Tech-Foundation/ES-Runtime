//! Assembly of the JS prelude (DECISIONS.md D8).
//!
//! Pure-JS web APIs ship as a prelude. D8 bakes it into a V8 startup snapshot;
//! for Phase 4 it is evaluated at runtime construction instead — identical
//! behavior, just slower context creation — with snapshot-baking landing in
//! Phase 8 (SPEC.md §6.8). Each fragment is a self-contained IIFE that installs
//! its globals on `globalThis`, using the host ops under `globalThis.__ops`.

/// The token `navigator.js` carries in place of the runtime's version, filled
/// in below so `navigator.userAgent` cannot drift from the binary reporting it.
const VERSION_TOKEN: &str = "__ES_RUNTIME_VERSION__";

/// The concatenated prelude source, in load order. `console` first so later
/// fragments (e.g. `globals`' `reportError`) can route through it.
pub(crate) fn source() -> String {
    [
        // Internal slot keys first: later fragments capture them at load time.
        include_str!("prelude/internals.js"),
        // DOMException next: later fragments throw it.
        include_str!("prelude/dom-exception.js"),
        // console before globals: reportError routes through it.
        include_str!("prelude/console.js"),
        include_str!("prelude/performance.js"),
        include_str!("prelude/navigator.js"),
        include_str!("prelude/globals.js"),
        include_str!("prelude/encoding.js"),
        include_str!("prelude/base64.js"),
        include_str!("prelude/structured-clone.js"),
        include_str!("prelude/url.js"),
        include_str!("prelude/urlpattern.js"),
        // events before abort: AbortSignal extends EventTarget.
        include_str!("prelude/events.js"),
        include_str!("prelude/abort.js"),
        // messaging needs EventTarget + MessageEvent (events) and structuredClone.
        include_str!("prelude/channel.js"),
        include_str!("prelude/streams.js"),
        // Transferable streams: needs the stream classes above, and the port
        // operations channel.js published.
        include_str!("prelude/transferable-streams.js"),
        // encoding streams need TransformStream + TextEncoder/TextDecoder.
        include_str!("prelude/encoding-streams.js"),
        // compression streams need TransformStream.
        include_str!("prelude/compression-streams.js"),
        // fetch family: blob before fetch (fetch bodies may be Blob/FormData).
        include_str!("prelude/blob.js"),
        include_str!("prelude/fetch.js"),
        // WebSocket (DECISIONS D29): needs events (MessageEvent/CloseEvent),
        // Blob, URL, DOMException — all installed above.
        include_str!("prelude/websocket.js"),
        include_str!("prelude/crypto.js"),
        // Hardening last: every global + host op must already be in place.
        include_str!("prelude/harden.js"),
    ]
    .join("\n")
    .replace(VERSION_TOKEN, env!("CARGO_PKG_VERSION"))
}

/// Prelude fragments that must run per-launch rather than being snapshot-baked.
///
/// A V8 snapshot-creator isolate does not expose `WebAssembly` on its global, so
/// `wasm.js` would find nothing to wrap at build time and its work would be
/// missing from every restored context. It runs after [`source`] — on the fresh
/// path directly, on the restored path against the deserialized context — where
/// the namespace and the engine's `__wasm_pending` builtin are both present.
/// `worker.js` is here for a different reason: which half of it installs
/// depends on whether *this* isolate is a worker, which the snapshot cannot
/// know — one blob is restored into both the agent driving the process and
/// every worker agent. It asks `__ops.worker_scope_info()` at launch instead.
pub(crate) fn post_snapshot_source() -> String {
    [
        include_str!("prelude/wasm.js"),
        include_str!("prelude/worker.js"),
    ]
    .join("\n")
}
