// Internal slot keys shared *between* prelude fragments (SPEC §4).
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
    }),
    writable: false,
    enumerable: false,
    configurable: false,
  });
})();
