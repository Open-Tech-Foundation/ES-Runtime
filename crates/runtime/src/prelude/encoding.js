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

  // Length of a trailing *incomplete but valid* UTF-8 sequence, i.e. how many
  // bytes must be held back until the next chunk arrives. An invalid lead byte
  // is not a prefix of anything, so it is decoded now (as U+FFFD) rather than
  // held forever.
  function incompleteTailLength(bytes) {
    const n = bytes.length;
    for (let back = 1; back <= 3 && back <= n; back++) {
      const b = bytes[n - back];
      if ((b & 0xc0) === 0x80) continue; // continuation byte: keep scanning back
      let needed;
      if ((b & 0x80) === 0) needed = 1;
      else if ((b & 0xe0) === 0xc0) needed = 2;
      else if ((b & 0xf0) === 0xe0) needed = 3;
      else if ((b & 0xf8) === 0xf0) needed = 4;
      else return 0; // invalid lead byte
      return back < needed ? back : 0;
    }
    return 0;
  }

  class TextDecoder {
    #fatal;
    #ignoreBOM;
    // Streaming state: bytes held back from a `{ stream: true }` call, and
    // whether the next decode is the start of a stream (only there is a BOM
    // stripped — it must not be re-stripped at every chunk boundary).
    #pending = null;
    #atStreamStart = true;

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

    decode(input, options = {}) {
      const streaming = Boolean(options && options.stream);
      let bytes = bytesOf(input);

      // Prepend anything held back from the previous streaming call.
      if (this.#pending !== null) {
        const joined = new Uint8Array(this.#pending.length + bytes.length);
        joined.set(this.#pending, 0);
        joined.set(bytes, this.#pending.length);
        bytes = joined;
        this.#pending = null;
      }

      if (streaming) {
        // Hold back a trailing sequence that is not yet complete, so a code
        // point split across chunks is not decoded as two replacements.
        const keep = incompleteTailLength(bytes);
        if (keep > 0) {
          this.#pending = bytes.slice(bytes.length - keep);
          bytes = bytes.subarray(0, bytes.length - keep);
        }
      }

      // A BOM belongs to the stream, not to each chunk.
      const ignoreBOM = this.#ignoreBOM || !this.#atStreamStart;
      // Rust validates/replaces and V8 builds the string natively.
      const text = ops.utf8_decode(bytes, this.#fatal, ignoreBOM);

      // A non-streaming call ends the stream: reset for the next one. (Any
      // bytes still held back were flushed above, becoming U+FFFD or, under
      // `fatal`, an error — both from the op.)
      this.#atStreamStart = !streaming;
      return text;
    }
  }

  for (const Interface of [TextEncoder, TextDecoder]) {
    Object.defineProperty(Interface.prototype, Symbol.toStringTag, {
      value: Interface.name,
      configurable: true,
    });
    globalThis[Interface.name] = Interface;
  }
})();
