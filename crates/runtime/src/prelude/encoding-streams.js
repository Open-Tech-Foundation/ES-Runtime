// TextEncoderStream / TextDecoderStream (SPEC §2.3), deferred from Phase 4 since
// they build on TransformStream. Both handle multi-unit sequences split across
// chunk boundaries (surrogate pairs when encoding; multi-byte UTF-8 when
// decoding).
(() => {
  "use strict";

  function toBytes(chunk) {
    if (chunk instanceof Uint8Array) return chunk;
    if (chunk instanceof ArrayBuffer) return new Uint8Array(chunk);
    if (ArrayBuffer.isView(chunk)) {
      return new Uint8Array(chunk.buffer, chunk.byteOffset, chunk.byteLength);
    }
    throw new TypeError("chunk must be a BufferSource");
  }
  function concat(a, b) {
    if (a.length === 0) return b;
    if (b.length === 0) return a;
    const out = new Uint8Array(a.length + b.length);
    out.set(a, 0);
    out.set(b, a.length);
    return out;
  }
  // Splits off a trailing incomplete UTF-8 sequence (held until more bytes).
  function splitCompleteUtf8(bytes) {
    let back = 0;
    while (back < 3 && bytes.length - 1 - back >= 0) {
      const b = bytes[bytes.length - 1 - back];
      if ((b & 0xc0) === 0x80) {
        back++;
        continue; // continuation byte
      }
      let needed;
      if (b < 0x80) needed = 1;
      else if ((b & 0xe0) === 0xc0) needed = 2;
      else if ((b & 0xf0) === 0xe0) needed = 3;
      else if ((b & 0xf8) === 0xf0) needed = 4;
      else needed = 1;
      const have = back + 1;
      if (have < needed) {
        const cut = bytes.length - have;
        return { complete: bytes.subarray(0, cut), rest: bytes.slice(cut) };
      }
      break;
    }
    return { complete: bytes, rest: new Uint8Array(0) };
  }

  class TextEncoderStream {
    #transform;
    constructor() {
      const encoder = new TextEncoder();
      let pendingHighSurrogate = "";
      this.#transform = new TransformStream({
        transform(chunk, controller) {
          let s = pendingHighSurrogate + String(chunk);
          pendingHighSurrogate = "";
          const last = s.charCodeAt(s.length - 1);
          if (last >= 0xd800 && last <= 0xdbff) {
            pendingHighSurrogate = s[s.length - 1];
            s = s.slice(0, -1);
          }
          if (s.length > 0) controller.enqueue(encoder.encode(s));
        },
        flush(controller) {
          if (pendingHighSurrogate) {
            // A leftover lone surrogate encodes as U+FFFD.
            controller.enqueue(encoder.encode(pendingHighSurrogate));
          }
        },
      });
    }
    get encoding() {
      return "utf-8";
    }
    get readable() {
      return this.#transform.readable;
    }
    get writable() {
      return this.#transform.writable;
    }
  }

  class TextDecoderStream {
    #transform;
    #encoding;
    constructor(label = "utf-8", options = {}) {
      const decoder = new TextDecoder(label, options); // validates the label
      this.#encoding = decoder.encoding;
      let pending = new Uint8Array(0);
      this.#transform = new TransformStream({
        transform(chunk, controller) {
          const combined = concat(pending, toBytes(chunk));
          const { complete, rest } = splitCompleteUtf8(combined);
          pending = rest;
          if (complete.length > 0) controller.enqueue(decoder.decode(complete));
        },
        flush(controller) {
          if (pending.length > 0) {
            const text = decoder.decode(pending);
            if (text) controller.enqueue(text);
          }
        },
      });
    }
    get encoding() {
      return this.#encoding;
    }
    get readable() {
      return this.#transform.readable;
    }
    get writable() {
      return this.#transform.writable;
    }
  }

  globalThis.TextEncoderStream = TextEncoderStream;
  globalThis.TextDecoderStream = TextDecoderStream;
})();
