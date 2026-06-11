// Headers / Request / Response / Body / fetch (SPEC §2.9). Networking goes
// through the host `fetch` op (the NetTransport provider, capability-gated);
// response bodies stream via `fetch_body_read`. CORS/cache/redirect modes that
// only apply in browsers are out of scope for a server-side runtime.
(() => {
  "use strict";
  const ops = globalThis.__ops;
  const encoder = new TextEncoder();
  const decoder = new TextDecoder();
  const BODY = Symbol("bodyState");

  // ---- Headers ------------------------------------------------------------

  function normalizeName(name) {
    const n = String(name);
    if (!/^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/.test(n)) {
      throw new TypeError(`Invalid header name: "${n}"`);
    }
    return n.toLowerCase();
  }
  function normalizeValue(value) {
    return String(value).replace(/^[\t\n\r ]+|[\t\n\r ]+$/g, "");
  }

  class Headers {
    #map = new Map(); // lowercased name -> [value, ...]
    constructor(init) {
      if (init === undefined || init === null) return;
      if (init instanceof Headers) {
        for (const [k, v] of init) this.append(k, v);
      } else if (Array.isArray(init)) {
        for (const pair of init) {
          if (pair.length !== 2) throw new TypeError("header pair must have 2 elements");
          this.append(pair[0], pair[1]);
        }
      } else if (typeof init === "object") {
        for (const k of Object.keys(init)) this.append(k, init[k]);
      }
    }
    append(name, value) {
      const n = normalizeName(name);
      const v = normalizeValue(value);
      const list = this.#map.get(n);
      if (list) list.push(v);
      else this.#map.set(n, [v]);
    }
    set(name, value) {
      this.#map.set(normalizeName(name), [normalizeValue(value)]);
    }
    get(name) {
      const list = this.#map.get(normalizeName(name));
      return list ? list.join(", ") : null;
    }
    getSetCookie() {
      return (this.#map.get("set-cookie") ?? []).slice();
    }
    has(name) {
      return this.#map.has(normalizeName(name));
    }
    delete(name) {
      this.#map.delete(normalizeName(name));
    }
    #sortedEntries() {
      const out = [];
      for (const [name, list] of this.#map) {
        out.push([name, name === "set-cookie" ? list : [list.join(", ")]]);
      }
      out.sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0));
      const flat = [];
      for (const [name, values] of out) {
        for (const v of values) flat.push([name, v]);
      }
      return flat;
    }
    *entries() {
      for (const e of this.#sortedEntries()) yield e;
    }
    *keys() {
      for (const [k] of this.#sortedEntries()) yield k;
    }
    *values() {
      for (const [, v] of this.#sortedEntries()) yield v;
    }
    forEach(cb, thisArg) {
      for (const [k, v] of this.#sortedEntries()) cb.call(thisArg, v, k, this);
    }
    [Symbol.iterator]() {
      return this.entries();
    }
    // Internal: flat list for the fetch op.
    _list() {
      return this.#sortedEntries();
    }
  }

  // ---- Body ---------------------------------------------------------------

  function makeBodyState(source) {
    // source: { bytes, stream, type } — at most one of bytes/stream.
    return { bytes: source.bytes ?? null, stream: source.stream ?? null, used: false };
  }
  function extractBody(input) {
    if (input === null || input === undefined) return { bytes: null, stream: null, type: null };
    if (typeof input === "string") {
      return { bytes: encoder.encode(input), type: "text/plain;charset=UTF-8" };
    }
    if (input instanceof Uint8Array) return { bytes: input };
    if (input instanceof ArrayBuffer) return { bytes: new Uint8Array(input) };
    if (ArrayBuffer.isView(input)) {
      return { bytes: new Uint8Array(input.buffer, input.byteOffset, input.byteLength) };
    }
    if (globalThis.Blob && input instanceof Blob) {
      return { bytes: input._bytes(), type: input.type || null };
    }
    if (globalThis.FormData && input instanceof FormData) {
      const enc = input._encode();
      return { bytes: enc.bytes, type: enc.type };
    }
    if (globalThis.URLSearchParams && input instanceof URLSearchParams) {
      return {
        bytes: encoder.encode(input.toString()),
        type: "application/x-www-form-urlencoded;charset=UTF-8",
      };
    }
    if (input instanceof ReadableStream) return { stream: input };
    return { bytes: encoder.encode(String(input)), type: "text/plain;charset=UTF-8" };
  }

  async function consumeBody(state) {
    if (state.used) throw new TypeError("Body has already been consumed");
    state.used = true;
    if (state.bytes !== null) return state.bytes;
    if (state.stream) {
      const reader = state.stream.getReader();
      const chunks = [];
      let total = 0;
      let x;
      while (!(x = await reader.read()).done) {
        chunks.push(x.value);
        total += x.value.length;
      }
      const out = new Uint8Array(total);
      let off = 0;
      for (const c of chunks) {
        out.set(c, off);
        off += c.length;
      }
      return out;
    }
    return new Uint8Array(0);
  }

  function defineBodyMixin(proto) {
    Object.defineProperties(proto, {
      bodyUsed: {
        configurable: true,
        get() {
          return this[BODY].used;
        },
      },
      body: {
        configurable: true,
        get() {
          const state = this[BODY];
          if (state.stream) return state.stream;
          if (state.bytes === null) return null;
          const bytes = state.bytes;
          let done = false;
          state.stream = new ReadableStream({
            pull(c) {
              if (!done) {
                done = true;
                c.enqueue(bytes.slice());
              } else c.close();
            },
          });
          return state.stream;
        },
      },
      arrayBuffer: {
        configurable: true,
        writable: true,
        value: async function () {
          const b = await consumeBody(this[BODY]);
          return b.buffer.slice(b.byteOffset, b.byteOffset + b.byteLength);
        },
      },
      bytes: {
        configurable: true,
        writable: true,
        value: async function () {
          return (await consumeBody(this[BODY])).slice();
        },
      },
      text: {
        configurable: true,
        writable: true,
        value: async function () {
          return decoder.decode(await consumeBody(this[BODY]));
        },
      },
      json: {
        configurable: true,
        writable: true,
        value: async function () {
          return JSON.parse(decoder.decode(await consumeBody(this[BODY])));
        },
      },
      blob: {
        configurable: true,
        writable: true,
        value: async function () {
          const b = await consumeBody(this[BODY]);
          return new Blob([b], { type: this.headers.get("content-type") || "" });
        },
      },
    });
  }

  // ---- Request ------------------------------------------------------------

  class Request {
    #method;
    #url;
    #headers;
    constructor(input, init = {}) {
      const options = init ?? {};
      if (input instanceof Request) {
        this.#method = options.method ? String(options.method).toUpperCase() : input.#method;
        this.#url = input.#url;
        this.#headers = new Headers(options.headers ?? input.#headers);
      } else {
        this.#url = new URL(String(input)).href;
        this.#method = options.method ? String(options.method).toUpperCase() : "GET";
        this.#headers = new Headers(options.headers);
      }
      const extracted =
        options.body !== undefined && options.body !== null
          ? extractBody(options.body)
          : { bytes: null, stream: null, type: null };
      if (extracted.type && !this.#headers.has("content-type")) {
        this.#headers.set("content-type", extracted.type);
      }
      this[BODY] = makeBodyState(extracted);
    }
    get method() {
      return this.#method;
    }
    get url() {
      return this.#url;
    }
    get headers() {
      return this.#headers;
    }
    clone() {
      return new Request(this);
    }
    // Internal accessors for fetch.
    _headers() {
      return this.#headers._list();
    }
  }
  defineBodyMixin(Request.prototype);

  // ---- Response -----------------------------------------------------------

  class Response {
    #status;
    #statusText;
    #headers;
    #url;
    constructor(body = null, init = {}) {
      const options = init ?? {};
      this.#status = options.status ?? 200;
      this.#statusText = options.statusText ?? "";
      this.#headers = new Headers(options.headers);
      this.#url = options.url ?? "";
      const extracted =
        body !== null && body !== undefined
          ? extractBody(body)
          : { bytes: null, stream: null, type: null };
      if (extracted.type && !this.#headers.has("content-type")) {
        this.#headers.set("content-type", extracted.type);
      }
      this[BODY] = makeBodyState(extracted);
    }
    get status() {
      return this.#status;
    }
    get statusText() {
      return this.#statusText;
    }
    get ok() {
      return this.#status >= 200 && this.#status < 300;
    }
    get headers() {
      return this.#headers;
    }
    get url() {
      return this.#url;
    }
    get redirected() {
      return false;
    }
    get type() {
      return "default";
    }
    clone() {
      const r = new Response(null, {
        status: this.#status,
        statusText: this.#statusText,
        headers: this.#headers,
        url: this.#url,
      });
      r[BODY] = { ...this[BODY] };
      return r;
    }
    static json(data, init = {}) {
      const r = new Response(JSON.stringify(data), init);
      if (!r.headers.has("content-type")) {
        r.headers.set("content-type", "application/json");
      }
      return r;
    }
    static error() {
      return new Response(null, { status: 0 });
    }
  }
  defineBodyMixin(Response.prototype);

  // ---- fetch --------------------------------------------------------------

  async function fetch(input, init) {
    const request = new Request(input, init);
    const bodyBytes = request.bodyUsed ? null : await consumeBody(request[BODY]);
    const hasBody = bodyBytes && bodyBytes.length > 0;

    const args = [request.method, request.url, hasBody ? bodyBytes : null];
    for (const [name, value] of request._headers()) args.push(name, value);

    const metaJson = await ops.fetch(...args);
    const meta = JSON.parse(metaJson);

    const bodyId = meta.bodyId;
    const stream = new ReadableStream({
      async pull(controller) {
        const chunk = await ops.fetch_body_read(bodyId);
        if (chunk === null) controller.close();
        else controller.enqueue(chunk);
      },
    });

    return new Response(stream, {
      status: meta.status,
      statusText: meta.statusText,
      headers: meta.headers,
      url: meta.url,
    });
  }

  globalThis.Headers = Headers;
  globalThis.Request = Request;
  globalThis.Response = Response;
  globalThis.fetch = fetch;
})();
