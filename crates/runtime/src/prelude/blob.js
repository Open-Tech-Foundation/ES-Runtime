// Blob / File / FormData (SPEC §2.9). Pure JS over Uint8Array.
(() => {
  "use strict";
  const encoder = new TextEncoder();
  const decoder = new TextDecoder();
  const BYTES = __internal.bytes;
  const ENCODE = __internal.encode;

  // A WebIDL iterable's iterator is its own interface: it must report
  // "[object FormData Iterator]" and inherit from %IteratorPrototype%, which a
  // bare generator object does not do.
  const ITER_GEN = Symbol("iterator generator");
  const ITERATOR_PROTOTYPE = Object.getPrototypeOf(
    Object.getPrototypeOf([][Symbol.iterator]()),
  );
  const FORMDATA_ITERATOR_PROTOTYPE = Object.create(ITERATOR_PROTOTYPE, {
    [Symbol.toStringTag]: {
      value: "FormData Iterator",
      configurable: true,
    },
    next: {
      value() {
        return this[ITER_GEN].next();
      },
      writable: true,
      configurable: true,
    },
  });
  function formDataIterator(generator) {
    const it = Object.create(FORMDATA_ITERATOR_PROTOTYPE);
    Object.defineProperty(it, ITER_GEN, { value: generator });
    return it;
  }

  function partToBytes(part) {
    if (part instanceof Blob) return part[BYTES]();
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

  // A MIME type is only kept if it parses: `type/subtype` in HTTP token
  // characters, optionally followed by parameters, all printable ASCII.
  // Anything else is dropped to "" rather than echoed back.
  const MIME_RE = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+\/[!#$%&'*+\-.^_`|~0-9A-Za-z]+([ \t]*;.*)?$/;
  function normalizeType(value) {
    if (value === undefined || value === null) return "";
    const s = String(value);
    // Any C0 control or non-ASCII byte disqualifies it outright.
    for (let i = 0; i < s.length; i++) {
      const c = s.charCodeAt(i);
      if (c < 0x20 || c > 0x7e) return "";
    }
    return MIME_RE.test(s) ? s.toLowerCase() : "";
  }
  // `endings: "native"` normalises CRLF and CR to the platform newline. This
  // runtime targets unix-like hosts, where that is LF.
  function normalizeEndings(parts) {
    return parts.map((p) =>
      typeof p === "string" ? p.replace(/\r\n|\r/g, "\n") : p,
    );
  }

  class Blob {
    #bytes;
    #type;
    constructor(parts = [], options = {}) {
      let list;
      if (parts === undefined || parts === null) {
        list = [];
      } else if (typeof parts !== "object" || typeof parts[Symbol.iterator] !== "function") {
        // Array.from would happily turn a number into [], hiding the mistake.
        throw new TypeError("Blob parts must be an iterable sequence");
      } else {
        list = [...parts];
      }
      const opts = options ?? {};
      if (opts.endings === "native") list = normalizeEndings(list);
      this.#bytes = concatBytes(list.map(partToBytes));
      this.#type = normalizeType(opts.type);
    }
    get size() {
      return this.#bytes.length;
    }
    get type() {
      return this.#type;
    }
    // Internal slot: raw bytes (used by FormData/fetch/structuredClone).
    [BYTES]() {
      return this.#bytes;
    }
    slice(start, end, contentType) {
      return new Blob([this.#bytes.slice(start, end)], { type: contentType });
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
    // Always empty: there is no directory picker to populate it, but code that
    // reads it should get the spec's empty string rather than undefined.
    get webkitRelativePath() {
      return "";
    }
  }

  function toEntryValue(value, filename) {
    if (value instanceof Blob) {
      if (filename !== undefined && !(value instanceof File)) {
        return new File([value[BYTES]()], String(filename), { type: value.type });
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
    *#entriesGen() {
      for (const e of this.#list) yield [e[0], e[1]];
    }
    *#keysGen() {
      for (const e of this.#list) yield e[0];
    }
    *#valuesGen() {
      for (const e of this.#list) yield e[1];
    }
    entries() {
      return formDataIterator(this.#entriesGen());
    }
    keys() {
      return formDataIterator(this.#keysGen());
    }
    values() {
      return formDataIterator(this.#valuesGen());
    }
    forEach(cb, thisArg) {
      for (const [k, v] of this.#list) cb.call(thisArg, v, k, this);
    }
    [Symbol.iterator]() {
      return this.entries();
    }
    // Internal slot: encode as multipart/form-data; returns { bytes, type }.
    [ENCODE]() {
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
          body = value[BYTES]();
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

  // ---- Object URLs ---------------------------------------------------------
  //
  // `URL.createObjectURL` lives here rather than in url.js because it is a Blob
  // feature: url.js loads first and has no Blob to register. The store is
  // shared through __internal so fetch can resolve a blob: URL.
  const blobURLs = __internal.blobURLs;

  Object.defineProperty(URL, "createObjectURL", {
    value(obj) {
      if (!(obj instanceof Blob)) {
        throw new TypeError("createObjectURL requires a Blob or File");
      }
      // There is no origin here, so the serialization uses the opaque form.
      const url = `blob:null/${crypto.randomUUID()}`;
      blobURLs.set(url, obj);
      return url;
    },
    writable: true,
    enumerable: true,
    configurable: true,
  });

  Object.defineProperty(URL, "revokeObjectURL", {
    value(url) {
      // Revoking an unknown or already-revoked URL is a no-op, not an error.
      blobURLs.delete(String(url));
    },
    writable: true,
    enumerable: true,
    configurable: true,
  });

  // Structured clone: both are serializable by value, and both are host objects
  // as far as V8 is concerned — it would otherwise serialize them as plain
  // objects, dropping the class and the private byte store. `File.prototype`'s
  // tag shadows the inherited `Blob` one, so a File round-trips as a File.
  for (const Interface of [Blob, File]) {
    Object.defineProperty(Interface.prototype, __internal.hostClone, {
      value: Interface.name,
    });
  }
  __internal.hostCodecs.set("Blob", {
    write: (b) => __internal.hostCodec.pack({ type: b.type }, b[BYTES]()),
    read: (bytes) => {
      const { header, payload } = __internal.hostCodec.unpack(bytes);
      return new Blob([payload], { type: header.type });
    },
  });
  __internal.hostCodecs.set("File", {
    write: (f) =>
      __internal.hostCodec.pack(
        { type: f.type, name: f.name, lastModified: f.lastModified },
        f[BYTES](),
      ),
    read: (bytes) => {
      const { header, payload } = __internal.hostCodec.unpack(bytes);
      return new File([payload], header.name, {
        type: header.type,
        lastModified: header.lastModified,
      });
    },
  });

  for (const Interface of [Blob, File, FormData]) {
    Object.defineProperty(Interface.prototype, Symbol.toStringTag, {
      value: Interface.name,
      configurable: true,
    });
    globalThis[Interface.name] = Interface;
  }
})();
