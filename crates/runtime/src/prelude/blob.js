// Blob / File / FormData (SPEC §2.9). Pure JS over Uint8Array.
(() => {
  "use strict";
  const encoder = new TextEncoder();
  const decoder = new TextDecoder();

  function partToBytes(part) {
    if (part instanceof Blob) return part._bytes();
    if (typeof part === "string") return encoder.encode(part);
    if (part instanceof Uint8Array) return part;
    if (part instanceof ArrayBuffer) return new Uint8Array(part);
    if (ArrayBuffer.isView(part)) {
      return new Uint8Array(part.buffer, part.byteOffset, part.byteLength);
    }
    return encoder.encode(String(part));
  }
  function concatBytes(list) {
    let total = 0;
    for (const b of list) total += b.length;
    const out = new Uint8Array(total);
    let off = 0;
    for (const b of list) {
      out.set(b, off);
      off += b.length;
    }
    return out;
  }
  function bytesStream(bytes) {
    let done = false;
    return new ReadableStream({
      pull(controller) {
        if (!done) {
          done = true;
          controller.enqueue(bytes.slice());
        } else {
          controller.close();
        }
      },
    });
  }

  class Blob {
    #bytes;
    #type;
    constructor(parts = [], options = {}) {
      this.#bytes = concatBytes(Array.from(parts ?? [], partToBytes));
      this.#type = options.type ? String(options.type).toLowerCase() : "";
    }
    get size() {
      return this.#bytes.length;
    }
    get type() {
      return this.#type;
    }
    // Internal: raw bytes (used by FormData/fetch/File).
    _bytes() {
      return this.#bytes;
    }
    slice(start, end, contentType) {
      return new Blob([this.#bytes.slice(start, end)], {
        type: contentType ? String(contentType) : "",
      });
    }
    async text() {
      return decoder.decode(this.#bytes);
    }
    async bytes() {
      return this.#bytes.slice();
    }
    async arrayBuffer() {
      const b = this.#bytes;
      return b.buffer.slice(b.byteOffset, b.byteOffset + b.byteLength);
    }
    stream() {
      return bytesStream(this.#bytes);
    }
  }

  class File extends Blob {
    #name;
    #lastModified;
    constructor(parts, name, options = {}) {
      super(parts, options);
      if (arguments.length < 2) {
        throw new TypeError("File requires a name");
      }
      this.#name = String(name);
      this.#lastModified = options.lastModified ?? Date.now();
    }
    get name() {
      return this.#name;
    }
    get lastModified() {
      return this.#lastModified;
    }
  }

  function toEntryValue(value, filename) {
    if (value instanceof Blob) {
      if (filename !== undefined && !(value instanceof File)) {
        return new File([value._bytes()], String(filename), { type: value.type });
      }
      return value;
    }
    return String(value);
  }

  class FormData {
    #list = [];
    append(name, value, filename) {
      this.#list.push([String(name), toEntryValue(value, filename)]);
    }
    set(name, value, filename) {
      const n = String(name);
      const entry = toEntryValue(value, filename);
      let placed = false;
      this.#list = this.#list.filter(([k]) => {
        if (k !== n) return true;
        if (!placed) {
          placed = true;
          return true;
        }
        return false;
      });
      const hit = this.#list.find(([k]) => k === n);
      if (hit) hit[1] = entry;
      else this.#list.push([n, entry]);
    }
    get(name) {
      const hit = this.#list.find(([k]) => k === String(name));
      return hit ? hit[1] : null;
    }
    getAll(name) {
      return this.#list.filter(([k]) => k === String(name)).map(([, v]) => v);
    }
    has(name) {
      return this.#list.some(([k]) => k === String(name));
    }
    delete(name) {
      this.#list = this.#list.filter(([k]) => k !== String(name));
    }
    *entries() {
      for (const e of this.#list) yield [e[0], e[1]];
    }
    *keys() {
      for (const e of this.#list) yield e[0];
    }
    *values() {
      for (const e of this.#list) yield e[1];
    }
    forEach(cb, thisArg) {
      for (const [k, v] of this.#list) cb.call(thisArg, v, k, this);
    }
    [Symbol.iterator]() {
      return this.entries();
    }
    // Internal: encode as multipart/form-data; returns { bytes, type }.
    _encode() {
      const boundary =
        "----ESRuntimeFormBoundary" + Math.random().toString(16).slice(2);
      const segments = [];
      for (const [name, value] of this.#list) {
        let header = `--${boundary}\r\nContent-Disposition: form-data; name="${name}"`;
        let body;
        if (value instanceof Blob) {
          const filename = value instanceof File ? value.name : "blob";
          header += `; filename="${filename}"\r\nContent-Type: ${
            value.type || "application/octet-stream"
          }\r\n\r\n`;
          body = value._bytes();
        } else {
          header += "\r\n\r\n";
          body = encoder.encode(value);
        }
        segments.push(encoder.encode(header), body, encoder.encode("\r\n"));
      }
      segments.push(encoder.encode(`--${boundary}--\r\n`));
      return {
        bytes: concatBytes(segments),
        type: `multipart/form-data; boundary=${boundary}`,
      };
    }
  }

  globalThis.Blob = Blob;
  globalThis.File = File;
  globalThis.FormData = FormData;
})();
