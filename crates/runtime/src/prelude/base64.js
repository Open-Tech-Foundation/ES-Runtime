// atob / btoa (SPEC §2.3) — base64 over Latin-1 strings. The transcoding loops
// live in the host `base64_encode`/`base64_decode` ops (base64_ops.rs); a null
// result signals invalid input, surfaced as the spec's InvalidCharacterError.
(() => {
  "use strict";
  const ops = globalThis.__ops;

  globalThis.btoa = (data) => {
    const out = ops.base64_encode(String(data));
    if (out === null) {
      throw new DOMException(
        "The string to be encoded contains characters outside of the Latin1 range.",
        "InvalidCharacterError",
      );
    }
    return out;
  };

  globalThis.atob = (data) => {
    const out = ops.base64_decode(String(data));
    if (out === null) {
      throw new DOMException(
        "The string to be decoded is not correctly encoded.",
        "InvalidCharacterError",
      );
    }
    return out;
  };
})();
