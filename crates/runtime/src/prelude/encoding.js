// TextEncoder / TextDecoder (SPEC §2.3), UTF-8 only — the WinterTC baseline.
// Transcoding is delegated to the host `utf8_encode`/`utf8_decode` ops, which
// ride V8's native UTF-16↔UTF-8 conversion (far faster than the pure-JS
// code-point loop). The streaming variants (TextEncoderStream/TextDecoderStream)
// build on TransformStream in encoding-streams.js.
(() => {
  "use strict";
  const ops = globalThis.__ops;

  class TextEncoder {
    get encoding() {
      return "utf-8";
    }

    encode(input = "") {
      // V8 transcodes the string argument UTF-16 → UTF-8 (lone surrogates →
      // U+FFFD) as it crosses to the host op — exactly TextEncoder semantics.
      return ops.utf8_encode(String(input));
    }

    encodeInto(source, destination) {
      const encoded = this.encode(source);
      const n = Math.min(encoded.length, destination.length);
      destination.set(encoded.subarray(0, n));
      // `read` ≈ `written` here since we encode whole code points up to `n`.
      return { read: source.length, written: n };
    }
  }

  function bytesOf(input) {
    if (input === undefined) return new Uint8Array(0);
    if (input instanceof Uint8Array) return input;
    if (input instanceof ArrayBuffer) return new Uint8Array(input);
    if (ArrayBuffer.isView(input)) {
      return new Uint8Array(input.buffer, input.byteOffset, input.byteLength);
    }
    throw new TypeError("TextDecoder input must be a BufferSource");
  }

  class TextDecoder {
    #fatal;
    #ignoreBOM;

    constructor(label = "utf-8", options = {}) {
      const enc = String(label).trim().toLowerCase();
      if (enc !== "utf-8" && enc !== "utf8" && enc !== "unicode-1-1-utf-8") {
        // WinterTC baseline is UTF-8; other labels are not supported yet.
        throw new RangeError(`unsupported encoding label: ${label}`);
      }
      this.#fatal = Boolean(options.fatal);
      this.#ignoreBOM = Boolean(options.ignoreBOM);
    }

    get encoding() {
      return "utf-8";
    }
    get fatal() {
      return this.#fatal;
    }
    get ignoreBOM() {
      return this.#ignoreBOM;
    }

    decode(input) {
      // Rust validates/replaces and V8 builds the string natively.
      return ops.utf8_decode(bytesOf(input), this.#fatal, this.#ignoreBOM);
    }
  }

  globalThis.TextEncoder = TextEncoder;
  globalThis.TextDecoder = TextDecoder;
})();
