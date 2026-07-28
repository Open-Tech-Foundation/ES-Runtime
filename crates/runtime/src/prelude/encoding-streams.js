// TextEncoderStream / TextDecoderStream (SPEC §2.3), deferred from Phase 4 since
// they build on TransformStream. Both handle multi-unit sequences split across
// chunk boundaries (surrogate pairs when encoding; multi-byte UTF-8 when
// decoding).
(() => {
  "use strict";

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
      // TextDecoder itself carries the streaming state (held-back partial
      // sequences, one-shot BOM handling), so this is a thin adapter over it.
      this.#transform = new TransformStream({
        transform(chunk, controller) {
          const text = decoder.decode(chunk, { stream: true });
          if (text) controller.enqueue(text);
        },
        flush(controller) {
          // The final, non-streaming decode flushes any held-back bytes.
          const text = decoder.decode();
          if (text) controller.enqueue(text);
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

  for (const Interface of [TextEncoderStream, TextDecoderStream]) {
    Object.defineProperty(Interface.prototype, Symbol.toStringTag, {
      value: Interface.name,
      configurable: true,
    });
    globalThis[Interface.name] = Interface;
  }
})();
