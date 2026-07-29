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

  // A decoder abandoned mid-stream — created, fed a `{ stream: true }` chunk,
  // then dropped without a final decode — would leave its native context in the
  // host registry forever. Normal use frees it when the stream ends; this is the
  // backstop for the case where nothing ever ends it.
  const reclaim = new FinalizationRegistry((handle) => {
    ops.decoder_free(handle);
  });

  class TextDecoder {
    #encoding;
    #fatal;
    #ignoreBOM;
    // The native decoder for an in-flight stream, or null. It is allocated
    // lazily: a one-shot `decode(bytes)` — the common case by far — carries no
    // state across calls and so needs no context at all.
    #handle = null;

    constructor(label = "utf-8", options = {}) {
      // The label table is the spec's, resolved host-side: `latin1` is
      // windows-1252, `utf-16` is utf-16le, and an unknown label is a
      // RangeError, which the op raises.
      this.#encoding = ops.encoding_for_label(String(label));
      this.#fatal = Boolean(options.fatal);
      this.#ignoreBOM = Boolean(options.ignoreBOM);
    }

    get encoding() {
      return this.#encoding;
    }
    get fatal() {
      return this.#fatal;
    }
    get ignoreBOM() {
      return this.#ignoreBOM;
    }

    decode(input, options = {}) {
      const streaming = Boolean(options && options.stream);
      const bytes = bytesOf(input);

      // No stream in flight and none being started: decode and be done.
      if (!streaming && this.#handle === null) {
        return ops.decode_once(this.#encoding, bytes, this.#fatal, this.#ignoreBOM);
      }

      if (this.#handle === null) {
        this.#handle = ops.decoder_new(this.#encoding, this.#ignoreBOM);
        reclaim.register(this, this.#handle, this);
      }
      // The host decoder holds any incomplete sequence — a multi-byte character
      // split across chunks, or a shift state — so nothing needs to be held
      // back here. `last` tells it the stream is over, which is when a trailing
      // partial sequence stops being "wait for more" and becomes an error.
      const text = ops.decoder_decode(this.#handle, bytes, this.#fatal, !streaming);
      if (!streaming) this.#release();
      return text;
    }

    // Ends the stream: the native decoder is done, and the finalizer must not
    // free a handle that has already been freed (or, worse, reused).
    #release() {
      ops.decoder_free(this.#handle);
      reclaim.unregister(this);
      this.#handle = null;
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
