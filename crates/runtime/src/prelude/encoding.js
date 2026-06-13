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
      const s = String(source);
      const encoded = ops.utf8_encode(s);
      if (encoded.length <= destination.length) {
        destination.set(encoded);
        return { read: s.length, written: encoded.length };
      }
      // Truncate on a code-point boundary: back `written` off any UTF-8
      // continuation bytes (0b10xxxxxx) so only whole code points are written.
      let written = destination.length;
      while (written > 0 && (encoded[written] & 0xc0) === 0x80) written--;
      destination.set(encoded.subarray(0, written));
      // `read` is the count of UTF-16 code units consumed — the decoded
      // prefix's JS length (truncation only; the common path never pays this).
      const read = ops.utf8_decode(encoded.subarray(0, written), false, true)
        .length;
      return { read, written };
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
