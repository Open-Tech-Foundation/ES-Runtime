// runtime:http — an HTTP/1.1 + HTTP/2 server: `serve((request, info) => response)`.
// The version is the client's to choose and the handler never sees it: over TLS
// it is ALPN (`h2` and `http/1.1` are both offered unless `alpn` narrows it),
// and on a cleartext port an HTTP/2 client is served h2c by prior knowledge. The handler
// is called with a web `Request` and returns (or resolves to) a web `Response`
// — the same Fetch API objects `fetch` uses. Backed by async ops over a vetted
// HTTP backend, gated on NetListen (binding the listening socket). Bodies
// stream in both directions: the request body is a `ReadableStream` pulling
// chunks from the host as they arrive, and a `ReadableStream` response body is
// pumped out chunk-by-chunk with backpressure (chunked transfer-encoding) —
// neither is materialized unless the handler asks (e.g. `request.text()`).

const ops = globalThis.__ops;
// Builds a Request from the host-validated absolute URL without re-parsing it.
const makeServerRequest = globalThis.__serverRequest;

function parseAddress(options) {
  const o = options ?? {};
  return {
    hostname: o.hostname ?? o.host ?? "0.0.0.0",
    port: Number(o.port) || 0,
  };
}

// TLS options, in the same shape runtime:net `listen` takes them (D28): the
// cert and key travel inline rather than as paths, because reading a file is
// the filesystem's privilege — a guest serving HTTPS from a cert on disk reads
// it with runtime:fs under its own gate, and serving needs nothing beyond
// NetListen.
function parseTls(options) {
  const o = options ?? {};
  if (o.secureTransport === undefined || o.secureTransport === "off") return null;
  if (o.secureTransport !== "on") {
    throw new TypeError(
      `serve: secureTransport must be "on" or "off", got ${JSON.stringify(o.secureTransport)}`,
    );
  }
  // Failing here beats binding a port and then rejecting every handshake.
  if (!o.cert || !o.key) {
    throw new TypeError('serve: secureTransport "on" requires both cert and key (PEM)');
  }
  // Left empty, the host advertises ["h2", "http/1.1"] — the server speaks both.
  // Naming `alpn` narrows that, e.g. ["http/1.1"] for a client that mishandles h2.
  const alpn = o.alpn ?? [];
  if (!Array.isArray(alpn)) throw new TypeError("serve: alpn must be an array of strings");
  return { cert: o.cert, key: o.key, alpn: alpn.map(String) };
}

// When to give up on a connection that is not making progress. Each one is
// left `null` unless the guest names it, and `null` means "the host's default"
// — the numbers live in one place, on the Rust side, so the two cannot drift.
// A guest that names `null` explicitly means "off", which crosses as 0.
//
// These bound only connections that are *idle or stalled*: a request in flight
// and a response still streaming are never interrupted, however long they take.
//
// A request *body* is the one that cannot be judged on elapsed time alone — a
// large upload over a slow link takes as long as an attacker dribbling a byte a
// minute, and what separates them is how much they send while taking it. So
// `bodyRead` is the allowance a body starts with and `bodyMinRate` (bytes per
// second) is what arriving adds to it: an upload extends its own deadline,
// a dribbler cannot. `bodyMinRate: 0` earns nothing, making `bodyRead` flat.
function parseTimeouts(options) {
  const t = (options ?? {}).timeouts;
  if (t === undefined || t === null) return [null, null, null, null, null];
  if (typeof t !== "object") {
    throw new TypeError(`serve: timeouts must be an object, got ${typeof t}`);
  }
  return [
    one(t, "handshake"),
    one(t, "headerRead"),
    one(t, "h2KeepAlive"),
    one(t, "bodyRead"),
    rate(t, "bodyMinRate"),
  ];

  // A rate rather than a duration: `null` is not "off" here — a body that earns
  // nothing is what `0` says, and `bodyRead: null` is how the bound is removed.
  function rate(o, name) {
    const v = o[name];
    if (v === undefined || v === null) return null; // host default
    if (typeof v !== "number") {
      throw new TypeError(`serve: timeouts.${name} must be a number of bytes per second`);
    }
    if (!Number.isFinite(v) || v < 0) {
      throw new RangeError(
        `serve: timeouts.${name} must be a finite, non-negative number of bytes per second`,
      );
    }
    return v;
  }

  function one(o, name) {
    const v = o[name];
    if (v === undefined) return null; // host default
    if (v === null) return 0; // explicitly disabled
    if (typeof v !== "number") {
      throw new TypeError(`serve: timeouts.${name} must be a number of ms or null`);
    }
    if (!Number.isFinite(v) || v < 0) {
      throw new RangeError(`serve: timeouts.${name} must be a finite, non-negative number of ms`);
    }
    // 0 already means disabled on the wire, and a 0ms timeout would fire before
    // any peer could answer — so the two agree.
    return v;
  }
}

// The most connections to serve at once. Absent (`null` on the wire) means no
// limit, which is the default: the right number follows from the deployment's
// file-descriptor budget and the memory a connection costs, neither of which
// the runtime can read, and a cap guessed for you would throttle real traffic
// with no error anywhere to explain it.
function parseMaxConnections(options) {
  return count(options, "maxConnections");
}

// The most connections one *peer address* may hold at once. Absent means no
// limit, and for a sharper reason than the whole-server cap: the count is per
// address, so everything behind one NAT or one load balancer shares a budget —
// with a proxy in front, every connection has the same source and any cap here
// is a cap on the whole service. Only the deployment knows what sits in front
// of it.
//
// A connection over this is *refused*, where one over `maxConnections` waits: an
// excess there is legitimate traffic queueing for a slot, and an excess here is
// one client past its share, which is the hold the cap exists to prevent.
function parseMaxConnectionsPerIp(options) {
  return count(options, "maxConnectionsPerIp");
}

function count(options, name) {
  const value = (options ?? {})[name];
  if (value === undefined || value === null) return null;
  if (typeof value !== "number") {
    throw new TypeError(`serve: ${name} must be a number or null, got ${typeof value}`);
  }
  if (!Number.isInteger(value) || value < 1) {
    throw new RangeError(`serve: ${name} must be an integer of at least 1`);
  }
  return value;
}

// Trailers a handler attached with withTrailers(), keyed by the Response they
// belong to. A WeakMap rather than a property on the Response: trailers are not
// part of the Fetch API — no runtime implements them there — so a non-standard
// key on a standard object would mean code written here silently does nothing
// elsewhere, and code from elsewhere silently loses trailers here. The import
// is the honest place for the dependency to show.
const attachedTrailers = new WeakMap();

// Normalizes HeadersInit-ish input to flat [name, value, ...] pairs.
function trailerPairs(init) {
  const flat = [];
  if (init instanceof Headers || (init && typeof init.forEach === "function" && !Array.isArray(init))) {
    init.forEach((value, name) => flat.push(String(name), String(value)));
  } else if (Array.isArray(init)) {
    for (const pair of init) flat.push(String(pair[0]), String(pair[1]));
  } else if (init && typeof init === "object") {
    for (const name of Object.keys(init)) flat.push(name, String(init[name]));
  } else {
    throw new TypeError("withTrailers: trailers must be a Headers, an object, or an array of pairs");
  }
  return flat;
}

// Sends `trailers` after the response body — the header fields that cannot be
// known until the body has been produced, which is what a gRPC status is.
//
// `trailers` is a Headers, a plain object, an array of pairs, or a promise of
// one of those. Returns the same Response, so it reads as a wrapper at the
// point of return.
//
// On HTTP/2 these become a trailing HEADERS frame. On HTTP/1.1 the wire format
// only carries trailer fields that the response's `Trailer` header names, so one
// is added automatically when the names are known in time — which they are for
// everything except a promise attached to a streaming body, where the head has
// already gone out by the time the names exist.
function withTrailers(response, trailers) {
  if (!(response instanceof Response)) {
    throw new TypeError("withTrailers(response, trailers): response must be a Response");
  }
  if (trailers == null) {
    throw new TypeError("withTrailers(response, trailers): trailers must not be null");
  }
  attachedTrailers.set(response, trailers);
  return response;
}

// The header fields that arrived after a response's body — the other half of
// withTrailers(), for a client reading what a server sent.
//
// Resolves once the body has been read to its end, because that is when
// trailers are on the wire; a response whose body is never read settles this
// when the body is dropped rather than waiting forever. Anything that is not a
// fetch response, or that carried no trailers, gives an empty Headers.
//
// Not a property on Response for the same reason withTrailers is not an option
// on it: no runtime exposes trailers on the Fetch API, so a standard-looking
// accessor would be a portability trap.
async function trailersOf(response) {
  if (!(response instanceof Response)) {
    throw new TypeError("trailersOf(response): response must be a Response");
  }
  const read = globalThis.__responseTrailers;
  return typeof read === "function" ? read(response) : new Headers();
}

// Streams a Response's ReadableStream body to the host one chunk at a time.
// Each push awaits the bounded host channel (download backpressure); a guest
// stream error is forwarded so the in-flight response aborts the connection —
// the only honest signal once the status line is on the wire.
async function pumpResponseBody(stream, id, trailers) {
  let reader;
  try {
    reader = stream.getReader(); // throws on a locked/consumed stream
    for (;;) {
      const { value, done } = await reader.read();
      if (done) break;
      const chunk = __internal.toBodyChunk(value);
      const accepted = await ops.http_response_body_push(id, chunk);
      if (!accepted) break; // host receiver gone (client disconnected)
    }
    // The body is complete, so anything the trailers were waiting on has
    // happened: resolve them now and send them with the close.
    let pairs = null;
    if (trailers !== null && trailers !== undefined) {
      try {
        pairs = trailerPairs(await trailers);
      } catch {
        pairs = null; // a rejected or malformed trailer set is not worth failing the body over
      }
    }
    await ops.http_response_body_close(id, null, pairs);
  } catch (e) {
    // Aborting the connection is the honest signal — the status line is already
    // on the wire, so there is no status left to change and a clean close would
    // claim a truncated body was complete. But the abort reaches the *client*,
    // and left the server's own author nothing to go on: `serve` has no error
    // hook, so a handler whose stream yields a bad chunk saw a connection reset
    // and no reason for it. Reported so it surfaces where uncaught errors do.
    reportError(e);
    await ops.http_response_body_close(id, String((e && e.message) || e));
  } finally {
    if (reader) reader.releaseLock();
  }
}

// Backs `request.signal`: an AbortSignal that aborts when the client goes away
// before the response was handed over, so a handler doing expensive work can
// stop rather than finish something nobody will read. Called on the handler's
// first read of `.signal` (see __serverRequest), because the watch it starts
// holds a pending op for the life of the request and most handlers never ask.
//
// The op always settles — `false` once the response is delivered — so this
// cannot hold the driven loop open past the request it belongs to. The `catch`
// is for a server torn down mid-request: that is not a failure of the request,
// and must not surface as an unhandled rejection.
function watchDisconnect(requestId) {
  const controller = new AbortController();
  ops
    .http_request_disconnected(requestId)
    .then((gone) => {
      if (gone) {
        controller.abort(new DOMException("The client disconnected.", "AbortError"));
      }
    })
    .catch(() => {});
  return controller.signal;
}

// What the handler is told about the connection a request arrived on, as its
// second argument — the shape Deno.serve passes, so a handler ports either way.
//
// `remoteAddr` is the *socket* peer and only ever that: behind a reverse proxy
// it is the proxy. Resolving `X-Forwarded-For` to the original client is the
// deployment's call, because it takes knowing which hop to trust — a header
// anyone can send is not an identity until something says whose to believe.
// Null when the host has no peer to report, which is honest about not knowing
// rather than handing back an address-shaped object full of blanks.
function connectionInfo(host, port) {
  if (!host) return { remoteAddr: null };
  return { remoteAddr: { transport: "tcp", hostname: host, port } };
}

// Runs one request through the handler and writes the response back. Never
// throws: a handler error or a non-Response return becomes a 500. `entry` is the
// structured tuple from http_next_request: [requestId, method, url, hasBody,
// headers, peerHost, peerPort] (headers as [name, value] pairs) — no
// per-request JSON parse.
// Names what a handler returned, for the 500's report. Deliberately a shape,
// not the value: a handler's return can hold anything, including secrets.
function describeReturn(v) {
  if (v === null) return "null";
  if (v === undefined) return "undefined";
  const t = typeof v;
  if (t === "object") {
    const name = v.constructor?.name;
    return !name || name === "Object" ? "a plain object" : `a ${name}`;
  }
  return `a ${t}`;
}

// Runs one request under a fresh root mapping carrying its own trace id.
//
// A no-op unless the program loaded `runtime:context`: until then nothing can
// observe a trace, the engine's promise hook is not installed so a mapping would
// not survive the first `await` anyway, and minting an id per request would be
// entropy drawn for nobody. `runtime:http` deliberately does not *import*
// `runtime:context` — that would switch the hook on for every server in the
// runtime (DECISIONS.md D88).
function inRequestScope(headers, trustTraceHeaders, method, url, run) {
  if (!globalThis.__ctx_enabled()) return run();
  const inner = () => inRequestSpan(method, url, run);
  const trace = inboundTrace(headers, trustTraceHeaders);
  const scope = __internal.context.scope;
  // The program imported `runtime:context`, so it owns the mapping's shape —
  // `currentTask().kind` and the per-context values both come from there.
  if (scope !== undefined) return scope(trace, "http-request", inner);
  // It did not, which is the shape a deployment gets when it turned telemetry on
  // without changing any code. There is then nothing that can *read* a mapping,
  // so a fresh trace and a cleared frame is the whole of what a request scope
  // has to be, and the engine's own accessors are enough to install it.
  return rootScope(trace ?? mintTrace(), inner);
}

// A request root built from the engine's accessors alone, for when
// `runtime:context` was never loaded.
function rootScope(traceId, run) {
  const savedTrace = globalThis.__ctx_swap_trace(traceId);
  const savedFrame = globalThis.__ctx_swap(undefined);
  try {
    return run();
  } finally {
    globalThis.__ctx_swap(savedFrame);
    globalThis.__ctx_swap_trace(savedTrace);
  }
}

// A W3C trace id. The same 16 random bytes `runtime:context` mints, drawn the
// same ungated way, because a trace id is a correlation key and not a secret.
function mintTrace() {
  const bytes = ops.random_bytes(16);
  let hex = "";
  for (let i = 0; i < bytes.length; i++) hex += bytes[i].toString(16).padStart(2, "0");
  return hex;
}

// The path of an absolute URL, by slicing rather than by `new URL()`.
//
// Parsing is an **op**, so it would be recorded — and recorded *before* the
// request span exists, landing in the request's trace as a sibling of the
// request rather than a child of it. A span's first act should not be to make a
// record it cannot contain. The host only ever hands this an absolute URL, which
// is why slicing is safe here and would not be in general.
function pathOf(url) {
  const scheme = url.indexOf("://");
  if (scheme === -1) return url;
  const slash = url.indexOf("/", scheme + 3);
  if (slash === -1) return "/";
  const query = url.indexOf("?", slash);
  return query === -1 ? url.slice(slash) : url.slice(slash, query);
}

// The root span of an inbound request's trace, covering the whole handler
// including a streamed response — so every op the handler issues nests under it
// rather than arriving as a flat list an exporter cannot shape.
//
// The name is the method alone. OpenTelemetry's convention is `{method} {route}`
// and falls back to `{method}` when no route is known, which is always here:
// this runtime has no router, and putting the raw path in the name would make
// every distinct URL its own span name — the high-cardinality mistake that makes
// a trace backend unusable. The path travels as an attribute instead.
function inRequestSpan(method, url, run) {
  const [id, startedAt] = globalThis.__span_open();
  // Nothing is recording — no subscriber, no exporter.
  if (id === 0) return run();

  const parent = globalThis.__ctx_span();
  const traceId = globalThis.__ctx_trace() ?? null;
  const attributes = ["http.request.method", method, "url.path", pathOf(url)];
  let ended = false;
  const finish = (status) => {
    if (ended) return;
    ended = true;
    globalThis.__span_close(id, parent, method, startedAt, status, traceId, attributes);
  };

  // Active for the *synchronous* call only. What carries it into the handler's
  // continuations is the capture each promise takes, so restoring here — before
  // the response is finished — is what keeps this request's span out of the
  // accept loop and out of the next request.
  const previous = globalThis.__ctx_swap_span(id);
  let result;
  try {
    result = run();
  } catch (e) {
    finish("error");
    throw e;
  } finally {
    globalThis.__ctx_swap_span(previous);
  }
  // `run()` hands back the handler's promise, so the span ends when the response
  // is complete rather than when the handler first yields.
  return Promise.resolve(result).then(
    (value) => {
      finish("ok");
      return value;
    },
    (error) => {
      finish("error");
      throw error;
    },
  );
}

async function handleRequest(entry, handler) {
  const requestId = entry[0];
  // Flipped once the response has been handed over, which is also when the
  // host stops holding anything for this request. Shared with the body stream
  // above, which outlives the handler whenever a handler keeps a reference.
  const answered = { done: false };
  const method = entry[1];
  const url = entry[2];
  const hasBody = entry[3];
  const headers = entry[4];
  const peerHost = entry[5];
  const peerPort = entry[6];
  let response;
  try {
    const init = { method, headers };
    // The body streams from the host chunk-by-chunk; nothing is buffered until
    // the handler consumes it. GET/HEAD must not carry a body in the Request
    // constructor (an unread host stream is dropped when the response ends).
    if (hasBody && method !== "GET" && method !== "HEAD") {
      init.body = new ReadableStream({
        async pull(controller) {
          // The host drops an undrained request body once the response is on
          // its way, so from here the body is over — reading it is not an
          // error, there is simply nothing more. Asking the host again would
          // name a request this agent has already given up (D50).
          if (answered.done) return controller.close();
          const chunk = await ops.http_body_read(requestId);
          if (chunk === null) controller.close();
          else controller.enqueue(chunk);
        },
      });
    }
    response = await handler(
      makeServerRequest(url, init, () => watchDisconnect(requestId), requestId),
      connectionInfo(peerHost, peerPort),
    );
    if (!(response instanceof Response)) {
      // A handler that returns something else has a bug, and coercing it with
      // `String(value)` shipped that bug as a 200: `return { ok: true }` went
      // out as the body "[object Object]", successfully. It is a 500, and the
      // reason is reported so it is not invisible — the response itself says
      // nothing, since a handler's mistake is not the client's business.
      reportError(
        new TypeError(
          `runtime:http handler returned ${describeReturn(response)} instead of a Response`,
        ),
      );
      response = new Response("Internal Server Error", { status: 500 });
    }
  } catch (e) {
    // Same reasoning: a thrown handler is a 500 to the client and a reported
    // error to the developer, rather than a silent one.
    reportError(e);
    response = new Response("Internal Server Error", { status: 500 });
  }

  // Fast path: hand a buffered body to http_respond without an async round-trip.
  // A deferred string body crosses as-is (encoded Rust-side — no utf8_encode op
  // and no intermediate JS byte buffer); already-materialized bytes pass
  // through. Only a streaming body goes through the chunk pump.
  const parts = response[__internal.parts]();
  let out = null;
  let stream = null;
  if (parts.str !== null && parts.str !== undefined) {
    out = parts.str;
  } else if (parts.bytes !== null) {
    out = parts.bytes;
  } else if (parts.stream) {
    stream = parts.stream;
  }
  const streamId = stream ? ops.http_response_body_new() : null;

  // Trailers attached with withTrailers(). A buffered body is already complete,
  // so they can be awaited here and cross with the response; a streamed body's
  // cannot, so `true` tells the host to expect them at close time.
  let trailers = attachedTrailers.get(response);
  let trailerArg = null;
  if (trailers !== undefined) {
    if (stream) {
      trailerArg = true;
    } else {
      try {
        trailerArg = trailerPairs(await trailers);
      } catch {
        trailerArg = null;
      }
    }
  }

  // A **buffered** response is complete, so the host gives the request up as it
  // sends it — nothing can still read the body. A **streamed** one is different
  // precisely because it may be echoing that body (`new Response(request.body)`),
  // so the request lives until the pump ends. This mirrors where the host
  // releases the id, and the two must agree or one of them is wrong.
  if (stream === null) answered.done = true;
  const args = [requestId, parts.status, out, streamId, trailerArg];
  // HTTP/1.1 sends only the trailer fields named in a `Trailer` header, so add
  // one when the names are known and the handler did not declare them itself.
  // HTTP/2 needs nothing here — this is the one place the two versions differ.
  const declared = Array.isArray(trailerArg) && trailerArg.length > 0;
  const hasTrailerHeader = parts.headers.some(([name]) => name.toLowerCase() === "trailer");
  for (const [name, value] of parts.headers) args.push(name, value);
  if (declared && !hasTrailerHeader) {
    const names = [];
    for (let i = 0; i < trailerArg.length; i += 2) names.push(trailerArg[i]);
    args.push("trailer", names.join(", "));
  }
  // Fire-and-forget: the response is dispatched on this op; not awaiting saves a
  // microtask/tick per request. http_respond only sends on a oneshot (never
  // rejects), so there is no rejection to surface. For a streaming body the
  // status/headers go out now and the chunks flow behind them via the pump.
  ops.http_respond(...args);
  if (stream) {
    await pumpResponseBody(stream, streamId, trailers);
    answered.done = true;
  }
}

// The handle returned by serve(): `addr` resolves to the bound address,
// `finished` resolves when the accept loop ends, `stop()` shuts it down.
class Server {
  constructor(
    hostname,
    port,
    tls,
    timeouts,
    maxConnections,
    maxConnectionsPerIp,
    reusePort,
    trustTraceHeaders,
    handler,
  ) {
    let resolveAddr, rejectAddr, resolveFinished, rejectFinished;
    this.addr = new Promise((res, rej) => {
      resolveAddr = res;
      rejectAddr = rej;
    });
    this.finished = new Promise((res, rej) => {
      resolveFinished = res;
      rejectFinished = rej;
    });
    this._id = null;
    this._stopped = false;

    (async () => {
      let info;
      try {
        // The ALPN list takes the whole argument tail, so everything else —
        // including the timeouts — is passed positionally before it.
        info = tls
          ? await ops.http_serve(
              hostname,
              port,
              tls.cert,
              tls.key,
              ...timeouts,
              maxConnections,
              maxConnectionsPerIp,
              reusePort,
              ...tls.alpn,
            )
          : await ops.http_serve(
              hostname,
              port,
              null,
              null,
              ...timeouts,
              maxConnections,
              maxConnectionsPerIp,
              reusePort,
            );
      } catch (e) {
        // A server that never bound has not "finished" — resolving `finished`
        // here made a failed bind indistinguishable from a clean shutdown, so
        // `await server.finished` returned normally and the program carried on
        // as though it had served. Both promises reject with the same error.
        rejectAddr(e);
        rejectFinished(e);
        // `finished` is marked handled so one failure is reported once: `addr`
        // is the promise that answers "did it bind", so that is the one left
        // for the unhandled-rejection path when nobody is watching. A program
        // that *does* await `finished` still sees the rejection — this only
        // suppresses the duplicate report, not the error.
        this.finished.catch(() => {});
        return;
      }
      this._id = info.id;
      resolveAddr({ hostname: info.localAddress, port: info.localPort });

      while (!this._stopped) {
        const flat = await ops.http_next_request(this._id);
        if (flat === null) break; // server closed
        
        let i = 0;
        while (i < flat.length) {
          const requestId = flat[i++];
          const method = flat[i++];
          const url = flat[i++];
          const hasBody = flat[i++];
          const peerHost = flat[i++];
          const peerPort = flat[i++];
          const numHeaders = flat[i++];
          
          const headers = [];
          for (let j = 0; j < numHeaders; j++) {
            headers.push([flat[i++], flat[i++]]);
          }
          
          // Handle each concurrently, each in its own async-context scope so
          // that `currentTask().traceId` — and anything the program's own
          // contexts put there — belongs to this request and not to the accept
          // loop that dispatched it.
          const entry = [requestId, method, url, hasBody, headers, peerHost, peerPort];
          inRequestScope(headers, trustTraceHeaders, method, url, () =>
            handleRequest(entry, handler),
          );
        }
      }
      resolveFinished();
    })();
  }

  async stop() {
    this._stopped = true;
    if (this._id !== null) await ops.http_close(this._id);
    await this.finished;
  }
}

// serve(handler) | serve(options, handler). Returns a Server immediately; the
// accept loop starts in the background.
// `SO_REUSEPORT`: let several processes bind this same port and have the kernel
// balance connections across them — how a server is run across cores without a
// front proxy, and how one is replaced without dropping connections. Unix-only;
// the host refuses it elsewhere rather than binding exclusively and leaving the
// caller to find out when a second process cannot start.
function parseReusePort(options) {
  const value = options.reusePort;
  if (value === undefined) return false;
  if (typeof value !== "boolean") {
    throw new TypeError(`serve: reusePort must be a boolean, got ${typeof value}`);
  }
  return value;
}

// W3C trace-context: `traceparent: 00-<32 hex trace-id>-<16 hex span-id>-<flags>`.
// Only the trace id is read; this runtime does not join a remote span.
const TRACEPARENT = /^[0-9a-f]{2}-([0-9a-f]{32})-[0-9a-f]{16}-[0-9a-f]{2}$/;

// Whether to believe an inbound `traceparent`. **Off by default**: the header
// arrives from whoever opened the connection, and a trace id is a correlation
// key that lands in logs and in every downstream request this one makes. Trusted
// by default, any client could stitch its requests into another tenant's trace,
// or replay one id across millions of requests and make a whole trace tree
// useless. Deny-by-default on untrusted input is the rule the rest of this
// runtime is built on, and an inbound header is untrusted input.
//
// Turn it on only behind a proxy that overwrites the header — which is what a
// mesh sidecar or an ingress controller does — and where the port is not
// reachable from outside it.
function parseTrustTraceHeaders(options) {
  const value = options.trustTraceHeaders;
  if (value === undefined) return false;
  if (typeof value !== "boolean") {
    throw new TypeError(
      `serve: trustTraceHeaders must be a boolean, got ${typeof value}`,
    );
  }
  return value;
}

// The trace id to run a request under, or null to mint a fresh one.
function inboundTrace(headers, trust) {
  if (!trust) return null;
  for (let i = 0; i < headers.length; i++) {
    if (headers[i][0].toLowerCase() !== "traceparent") continue;
    const match = TRACEPARENT.exec(headers[i][1].trim().toLowerCase());
    // A malformed or all-zero id is refused rather than propagated: it is the
    // usual shape of a field that was never filled in.
    if (match === null || /^0+$/.test(match[1])) return null;
    return match[1];
  }
  return null;
}

function serve(options, handler) {
  if (typeof options === "function") {
    handler = options;
    options = {};
  }
  if (typeof handler !== "function") {
    throw new TypeError("serve(options, handler): handler must be a function");
  }
  const { hostname, port } = parseAddress(options);
  // Parsed before the bind, so a bad option is a TypeError rather than a port
  // that is claimed and then abandoned.
  return new Server(
    hostname,
    port,
    parseTls(options),
    parseTimeouts(options),
    parseMaxConnections(options),
    parseMaxConnectionsPerIp(options),
    parseReusePort(options),
    parseTrustTraceHeaders(options),
    handler,
  );
}

export { serve, withTrailers, trailersOf };
export default { serve, withTrailers, trailersOf };
