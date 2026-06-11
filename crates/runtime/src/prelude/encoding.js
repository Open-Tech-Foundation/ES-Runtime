// TextEncoder / TextDecoder (SPEC §2.3), UTF-8 only — the WinterTC baseline.
// Implemented in pure JS over Uint8Array; a zero-copy Rust path is a Phase 8
// optimization. The streaming variants (TextEncoderStream/TextDecoderStream)
// need TransformStream and land in Phase 5 (SPEC §7).
(() => {
  "use strict";

  class TextEncoder {
    get encoding() {
      return "utf-8";
    }

    encode(input = "") {
      const str = String(input);
      const out = [];
      for (let i = 0; i < str.length; i++) {
        let code = str.charCodeAt(i);
        // Combine a surrogate pair into a single code point.
        if (code >= 0xd800 && code <= 0xdbff && i + 1 < str.length) {
          const next = str.charCodeAt(i + 1);
          if (next >= 0xdc00 && next <= 0xdfff) {
            code = 0x10000 + ((code - 0xd800) << 10) + (next - 0xdc00);
            i++;
          }
        }
        // A lone surrogate is replaced with U+FFFD per spec.
        if (code >= 0xd800 && code <= 0xdfff) code = 0xfffd;

        if (code < 0x80) {
          out.push(code);
        } else if (code < 0x800) {
          out.push(0xc0 | (code >> 6), 0x80 | (code & 0x3f));
        } else if (code < 0x10000) {
          out.push(
            0xe0 | (code >> 12),
            0x80 | ((code >> 6) & 0x3f),
            0x80 | (code & 0x3f),
          );
        } else {
          out.push(
            0xf0 | (code >> 18),
            0x80 | ((code >> 12) & 0x3f),
            0x80 | ((code >> 6) & 0x3f),
            0x80 | (code & 0x3f),
          );
        }
      }
      return new Uint8Array(out);
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
      const bytes = bytesOf(input);
      let i = 0;
      // Skip a leading BOM unless asked to keep it.
      if (
        !this.#ignoreBOM &&
        bytes.length >= 3 &&
        bytes[0] === 0xef &&
        bytes[1] === 0xbb &&
        bytes[2] === 0xbf
      ) {
        i = 3;
      }
      let result = "";
      while (i < bytes.length) {
        const b0 = bytes[i];
        let code;
        let size;
        if (b0 < 0x80) {
          code = b0;
          size = 1;
        } else if ((b0 & 0xe0) === 0xc0) {
          code = b0 & 0x1f;
          size = 2;
        } else if ((b0 & 0xf0) === 0xe0) {
          code = b0 & 0x0f;
          size = 3;
        } else if ((b0 & 0xf8) === 0xf0) {
          code = b0 & 0x07;
          size = 4;
        } else {
          if (this.#fatal) throw new TypeError("invalid UTF-8");
          result += "�";
          i++;
          continue;
        }
        if (i + size > bytes.length) {
          if (this.#fatal) throw new TypeError("truncated UTF-8");
          result += "�";
          break;
        }
        let valid = true;
        for (let k = 1; k < size; k++) {
          const b = bytes[i + k];
          if ((b & 0xc0) !== 0x80) {
            valid = false;
            break;
          }
          code = (code << 6) | (b & 0x3f);
        }
        if (!valid) {
          if (this.#fatal) throw new TypeError("invalid UTF-8");
          result += "�";
          i++;
          continue;
        }
        i += size;
        result += String.fromCodePoint(code);
      }
      return result;
    }
  }

  globalThis.TextEncoder = TextEncoder;
  globalThis.TextDecoder = TextDecoder;
})();
