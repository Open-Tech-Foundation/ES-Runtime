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
  // Closure-private marker for responses the runtime builds itself (a network
  // response, `Response.error()`): these are *internal* responses in Fetch
  // terms and are not subject to the constructor's status/body checks, which
  // only constrain what a script may construct.
  const INTERNAL_RESPONSE = Symbol("internalResponse");

  // Statuses that must not carry a body (Fetch: "null body status").
  const NULL_BODY_STATUS = new Set([101, 103, 204, 205, 304]);
  // Statuses `Response.redirect` accepts (Fetch: "redirect status").
  const REDIRECT_STATUS = new Set([301, 302, 303, 307, 308]);

  // ---- Headers ------------------------------------------------------------

  function normalizeName(name) {
    const n = String(name);
    if (!/^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/.test(n)) {
      throw new TypeError(`Invalid header name: "${n}"`);
    }
    return n.toLowerCase();
  }
  function normalizeValue(value) {
    // Strip leading/trailing HTTP whitespace first, then reject anything that
    // is not a header value byte. NUL/CR/LF *inside* the value would otherwise
    // let a caller splice extra header lines (or a body) into the wire format
    // via any header built from untrusted input — request/response splitting.
    const v = String(value).replace(/^[\t\n\r ]+|[\t\n\r ]+$/g, "");
    if (/[\0\n\r]/.test(v)) {
      throw new TypeError("Invalid header value: contains NUL, CR or LF");
    }
    return v;
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
    #signal;
    constructor(input, init = {}) {
      const options = init ?? {};
      if (options.signal !== undefined && options.signal !== null) {
        if (!(options.signal instanceof AbortSignal)) {
          throw new TypeError("Request signal must be an AbortSignal");
        }
        this.#signal = options.signal;
      } else if (input instanceof Request) {
        this.#signal = input.#signal;
      } else {
        // Every request has a signal, even when the caller passed none.
        this.#signal = new AbortController().signal;
      }
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
    get signal() {
      return this.#signal;
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
    #type = "default";
    constructor(body = null, init = {}) {
      const options = init ?? {};
      const internal = options[INTERNAL_RESPONSE] === true;
      const status = options.status ?? 200;
      if (!internal) {
        if (!Number.isInteger(status) || status < 200 || status > 599) {
          throw new RangeError(`Response status ${status} is outside 200-599`);
        }
        if (NULL_BODY_STATUS.has(status) && body !== null && body !== undefined) {
          throw new TypeError(`Response with a null body status (${status}) cannot have a body`);
        }
      }
      this.#status = status;
      this.#statusText = options.statusText ?? "";
      this.#headers = new Headers(options.headers);
      this.#url = options.url ?? "";
      if (options.type) this.#type = String(options.type);
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
      return this.#type;
    }
    clone() {
      const r = new Response(null, {
        status: this.#status,
        statusText: this.#statusText,
        headers: this.#headers,
        url: this.#url,
        type: this.#type,
        [INTERNAL_RESPONSE]: true,
      });
      r[BODY] = { ...this[BODY] };
      return r;
    }
    static json(data, init = {}) {
      const options = init ?? {};
      const serialized = JSON.stringify(data);
      if (serialized === undefined) {
        throw new TypeError("Response.json: the value is not JSON-serializable");
      }
      const r = new Response(serialized, options);
      // The string body already inferred "text/plain", so the JSON type has to
      // be set unless the *caller's* init supplied one of its own.
      if (!new Headers(options.headers).has("content-type")) {
        r.headers.set("content-type", "application/json");
      }
      return r;
    }
    static error() {
      return new Response(null, {
        status: 0,
        type: "error",
        [INTERNAL_RESPONSE]: true,
      });
    }
    static redirect(url, status = 302) {
      const location = new URL(String(url)).href;
      if (!REDIRECT_STATUS.has(status)) {
        throw new RangeError(`Response.redirect: ${status} is not a redirect status`);
      }
      return new Response(null, {
        status,
        headers: { location },
        [INTERNAL_RESPONSE]: true,
      });
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

  // Streams a request body's ReadableStream to the host one chunk at a time.
  // Each push awaits the bounded host channel (upload backpressure); a guest
  // stream error is forwarded so the in-flight request aborts cleanly.
  async function pumpRequestBody(stream, id) {
    const reader = stream.getReader();
    try {
      for (;;) {
        const { value, done } = await reader.read();
        if (done) break;
        let chunk;
        if (value instanceof Uint8Array) chunk = value;
        else if (ArrayBuffer.isView(value)) {
          chunk = new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
        } else if (value instanceof ArrayBuffer) chunk = new Uint8Array(value);
        else throw new TypeError("ReadableStream body must yield Uint8Array chunks");
        const accepted = await ops.fetch_request_body_push(id, chunk);
        if (!accepted) break; // host receiver gone (request finished/failed)
      }
      await ops.fetch_request_body_close(id);
    } catch (e) {
      await ops.fetch_request_body_close(id, String((e && e.message) || e));
      throw e;
    } finally {
      reader.releaseLock();
    }
  }

  async function fetch(input, init) {
    const request = new Request(input, init);
    const state = request[BODY];
    const signal = request.signal;

    // An already-aborted signal fails before anything touches the network.
    if (signal.aborted) throw signal.reason;

    // Wire the abort BEFORE any await. Body materialization below suspends, and
    // an abort landing in that window must not be missed — so the handle and the
    // listener are in place before the first suspension point. Firing the handle
    // drops the transport future host-side (the actual connection teardown);
    // the JS side owns the rejection *value*, since the spec rejects with the
    // signal's reason, which may be any value.
    const abortId = ops.fetch_abort_new();
    let onAbort = null;
    // `Promise.race` below attaches a handler to this, so an abort arriving
    // after the response is still handled and never becomes an unhandled
    // rejection. `catch` here keeps that true for the pre-race window too.
    const abortPromise = new Promise((_resolve, reject) => {
      onAbort = () => {
        ops.fetch_abort(abortId);
        reject(signal.reason);
      };
      signal.addEventListener("abort", onAbort, { once: true });
    });
    abortPromise.catch(() => {});

    // A ReadableStream body streams to the host without buffering; anything else
    // (bytes or a deferred string) is materialized and sent as one buffered body.
    let bodyStreamId = null;
    let bodyBytes = null;
    if (!state.used) {
      if (state.stream && state.bytes === null && state.str === null) {
        bodyStreamId = ops.fetch_request_body_new();
        state.used = true;
      } else {
        bodyBytes = await consumeBody(state);
      }
    }
    const hasBody = bodyBytes && bodyBytes.length > 0;

    // Materializing the body suspended; the signal may have fired meanwhile.
    if (signal.aborted) {
      signal.removeEventListener("abort", onAbort);
      throw signal.reason;
    }

    const args = [
      request.method,
      request.url,
      hasBody ? bodyBytes : null,
      bodyStreamId,
      abortId,
    ];
    for (const [name, value] of request._headers()) args.push(name, value);

    // Start the request and (for a streaming body) the pump concurrently — the
    // driven loop polls both, so chunks flow while `fetch` awaits the response.
    // The pump's rejection is captured rather than left floating: if the request
    // itself fails first, that error wins and the pump error must not surface as
    // an unhandled rejection.
    let pumpError = null;
    const fetchPromise = ops.fetch(...args);
    const pumpPromise =
      bodyStreamId !== null
        ? pumpRequestBody(state.stream, bodyStreamId).catch((e) => {
            pumpError = e;
          })
        : null;

    let meta;
    try {
      meta = await Promise.race([fetchPromise, abortPromise]);
    } finally {
      signal.removeEventListener("abort", onAbort);
    }
    if (pumpPromise) await pumpPromise;
    if (pumpError) throw pumpError;

    const bodyId = meta.bodyId;
    // An abort after the headers arrived errors the body stream and drops the
    // host-side stream, closing the connection rather than leaving it to drain.
    const stream = new ReadableStream({
      start(controller) {
        if (signal.aborted) {
          ops.fetch_body_cancel(bodyId);
          controller.error(signal.reason);
          return;
        }
        signal.addEventListener(
          "abort",
          () => {
            ops.fetch_body_cancel(bodyId);
            try {
              controller.error(signal.reason);
            } catch {
              // Already closed or errored — nothing to signal.
            }
          },
          { once: true },
        );
      },
      async pull(controller) {
        const chunk = await ops.fetch_body_read(bodyId);
        if (chunk === null) controller.close();
        else controller.enqueue(chunk);
      },
      cancel() {
        ops.fetch_body_cancel(bodyId);
      },
    });

    return new Response(stream, {
      status: meta.status,
      statusText: meta.statusText,
      headers: meta.headers,
      url: meta.url,
      [INTERNAL_RESPONSE]: true,
    });
  }

  for (const Interface of [Headers, Request, Response]) {
    Object.defineProperty(Interface.prototype, Symbol.toStringTag, {
      value: Interface.name,
      configurable: true,
    });
    globalThis[Interface.name] = Interface;
  }
  globalThis.fetch = fetch;
  // Internal bridge for runtime:http: build a server-side Request from a
  // host-validated absolute URL without the URL re-parse. Keyed by a private
  // symbol so only the prelude can grant the trust.
  Object.defineProperty(globalThis, "__serverRequest", {
    value: (url, init) => new Request(url, { ...init, [TRUSTED_URL]: true }),
  });
})();
