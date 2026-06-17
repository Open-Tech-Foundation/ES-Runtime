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
  // Closure-private marker: a Request built from an already-validated absolute
  // URL (the runtime:http server path) may skip re-parsing it. Not reachable
  // from guest code, so the public constructor's eager validation is unaffected.
  const TRUSTED_URL = Symbol("trustedUrl");

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
    // source: { bytes, str, stream, type } — at most one of bytes/str/stream.
    // `str` defers UTF-8 encoding (the utf8_encode op) until the body is read,
    // so a string body that is never consumed as bytes — or that crosses
    // straight to a host op that encodes Rust-side — pays nothing here.
    return {
      bytes: source.bytes ?? null,
      str: source.str ?? null,
      stream: source.stream ?? null,
      used: false,
    };
  }
  // Materializes a body state's bytes, encoding a deferred string on first read.
  function bodyBytes(state) {
    if (state.bytes === null && state.str !== null) {
      state.bytes = encoder.encode(state.str);
      state.str = null;
    }
    return state.bytes;
  }
  function extractBody(input) {
    if (input === null || input === undefined) return { bytes: null, stream: null, type: null };
    if (typeof input === "string") {
      // Deferred: keep the string; encode lazily (see bodyBytes).
      return { str: input, type: "text/plain;charset=UTF-8" };
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
    const bytes = bodyBytes(state);
    if (bytes !== null) return bytes;
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
          const bytes = bodyBytes(state);
          if (bytes === null) return null;
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
    // Deferred header init: in the trusted server path the headers are kept as a
    // raw [name, value] list and the Headers object is built only on first
    // access (#ensureHeaders) — a handler that never reads req.headers (e.g. a
    // plain hello-world) pays nothing for header normalization.
    #rawHeaders = null;
    constructor(input, init = {}) {
      const options = init ?? {};
      if (input instanceof Request) {
        this.#method = options.method ? String(options.method).toUpperCase() : input.#method;
        this.#url = input.#url;
        this.#headers = new Headers(options.headers ?? input.headers);
      } else if (options[TRUSTED_URL]) {
        // Internal server path (runtime:http): `input` is an absolute URL the
        // host already parsed and validated, so skip re-parsing it (the URL op);
        // defer building the Headers object until something reads it.
        this.#url = input;
        this.#method = options.method ? String(options.method).toUpperCase() : "GET";
        this.#headers = null;
        this.#rawHeaders = options.headers ?? null;
      } else {
        this.#url = new URL(String(input)).href;
        this.#method = options.method ? String(options.method).toUpperCase() : "GET";
        this.#headers = new Headers(options.headers);
      }
      const extracted =
        options.body !== undefined && options.body !== null
          ? extractBody(options.body)
          : { bytes: null, stream: null, type: null };
      if (extracted.type) {
        this.#ensureHeaders();
        if (!this.#headers.has("content-type")) {
          this.#headers.set("content-type", extracted.type);
        }
      }
      this[BODY] = makeBodyState(extracted);
    }
    #ensureHeaders() {
      if (this.#headers === null) {
        this.#headers = new Headers(this.#rawHeaders ?? undefined);
        this.#rawHeaders = null;
      }
    }
    get method() {
      return this.#method;
    }
    get url() {
      return this.#url;
    }
    get headers() {
      this.#ensureHeaders();
      return this.#headers;
    }
    clone() {
      return new Request(this);
    }
    // Internal accessors for fetch.
    _headers() {
      this.#ensureHeaders();
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
    // Internal (runtime:http): synchronous response parts, so the server can
    // skip the async arrayBuffer() round-trip for the common buffered body.
    // `bytes` is the body Uint8Array, or null for an absent body or a streaming
    // body (in which case `stream` is set and the caller drains it async).
    _parts() {
      const s = this[BODY];
      return {
        status: this.#status,
        headers: this.#headers._list(),
        // A deferred string body crosses to http_respond as-is (encoded
        // Rust-side); otherwise hand over already-materialized bytes or a stream.
        str: s.str,
        bytes: s.bytes,
        stream: s.stream,
      };
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

    const meta = await ops.fetch(...args);

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
  // Internal bridge for runtime:http: build a server-side Request from a
  // host-validated absolute URL without the URL re-parse. Keyed by a private
  // symbol so only the prelude can grant the trust.
  Object.defineProperty(globalThis, "__serverRequest", {
    value: (url, init) => new Request(url, { ...init, [TRUSTED_URL]: true }),
  });
})();
