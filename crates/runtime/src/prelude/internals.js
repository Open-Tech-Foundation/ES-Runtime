// Internal slot keys and shared state used *between* prelude fragments (SPEC §4).
//
// Each fragment is a separate IIFE, so a fragment-local `Symbol()` is genuinely
// unreachable from guest code and is the right choice for anything one fragment
// uses on its own. The three keys here are the exceptions — slots one fragment
// defines and another has to read:
//
//   bytes   Blob's backing Uint8Array   — blob.js  → fetch.js, structured-clone.js
//   encode  FormData multipart encoder  — blob.js  → fetch.js
//   parts   Response's synchronous parts — fetch.js → runtime_modules/http.js
//
// Plus shared state:
//
//   blobURLs   the object-URL store — blob.js registers, fetch.js resolves
//   hostCodecs the structured-clone codec table — see `hostClone` below
//
// `parts` is why this object survives rather than being deleted once the
// prelude has run: `runtime:http` is imported lazily, long after, and must be
// able to read the slot then.
//
// This binding is locked in harden.js the same way `__ops` is, and carries the
// same caveat: it is defense-in-depth for the JS surface, not the security
// boundary. Nothing here grants a capability — the op table and capability set
// live in the engine's Rust `OpState`, where guest code cannot reach them. What
// it buys is a clean WebIDL surface: these slots no longer appear as named
// members of a public prototype, where `Object.getOwnPropertyNames` and any
// enumeration of the interface would report them.
(() => {
  "use strict";
  Object.defineProperty(globalThis, "__internal", {
    value: Object.freeze({
      bytes: Symbol("Blob bytes"),
      encode: Symbol("FormData encode"),
      parts: Symbol("Response parts"),
      // href -> Blob, for URL.createObjectURL. Entries live until revoked;
      // there is no document unload to clear them.
      blobURLs: new Map(),

      // Structured clone's host-object seam. V8's serializer knows JS types,
      // not ours — Blob, File, DOMException, MessagePort — so those declare a
      // codec here and tag their prototype with `hostClone`.
      //
      // A *registered* symbol, because the engine names the same one from Rust
      // (`Symbol.for`, not a fresh `Symbol()`): the serializer's per-object test
      // is then a single property lookup in Rust rather than a call back into
      // JS for every object in the graph. The value is the codec's tag.
      //
      // Forging the tag on an arbitrary object gains nothing: the worst it can
      // do is miss the codec table and raise DataCloneError.
      hostClone: Symbol.for("es-runtime.hostClone"),
      // tag -> { write(object) -> Uint8Array, read(bytes) -> object }
      hostCodecs: new Map(),

      // The ports named in the transfer list of the serialization currently
      // running. The MessagePort codec consults it to tell a *transferred* port
      // from a *cloned* one: the spec allows the first and refuses the second,
      // and by the time the codec sees the object that is the only difference
      // between them.
      transferringPorts: new Set(),
      // The same, for streams: a ReadableStream/WritableStream/TransformStream
      // may be transferred and may not be cloned, and by the time the codec
      // runs the transfer list is the only thing that distinguishes them.
      transferringStreams: new Set(),
      // channel.js's "you have been transferred away" hook, called by
      // structured-clone.js once serialization has succeeded.
      portDetach: Symbol("MessagePort detach"),
      // Shared framing helper, filled in by structured-clone.js. Declared here
      // because the freeze below is shallow: a slot present at freeze time can
      // still be populated, where a new property on `__internal` could not.
      hostCodec: {},
      // structured-clone.js fills in `transfer.serialize`, the shared
      // transfer-list reading used by structuredClone, Worker.postMessage and
      // MessagePort.postMessage. An object for the same reason as `hostCodec`:
      // the freeze below is shallow, so a slot present now can be populated,
      // where a new property on `__internal` could not.
      transfer: {},
      // channel.js fills these in: `adopt(id)` wraps a transferred port id in a
      // MessagePort, `idOf(port)` reads one back. Transferable streams need
      // both — a stream is transferred *as* a port pair — and they cannot reach
      // channel.js's own constructor symbol.
      ports: {},
    }),
    writable: false,
    enumerable: false,
    configurable: false,
  });
})();
