// runtime:http — an HTTP/1.1 server: `serve((request) => response)`. The handler
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

// Streams a Response's ReadableStream body to the host one chunk at a time.
// Each push awaits the bounded host channel (download backpressure); a guest
// stream error is forwarded so the in-flight response aborts the connection —
// the only honest signal once the status line is on the wire.
async function pumpResponseBody(stream, id) {
  let reader;
  try {
    reader = stream.getReader(); // throws on a locked/consumed stream
    for (;;) {
      const { value, done } = await reader.read();
      if (done) break;
      let chunk;
      if (value instanceof Uint8Array) chunk = value;
      else if (ArrayBuffer.isView(value)) {
        chunk = new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
      } else if (value instanceof ArrayBuffer) chunk = new Uint8Array(value);
      else throw new TypeError("ReadableStream body must yield Uint8Array chunks");
      const accepted = await ops.http_response_body_push(id, chunk);
      if (!accepted) break; // host receiver gone (client disconnected)
    }
    await ops.http_response_body_close(id);
  } catch (e) {
    await ops.http_response_body_close(id, String((e && e.message) || e));
  } finally {
    if (reader) reader.releaseLock();
  }
}

// Runs one request through the handler and writes the response back. Never
// throws: a handler error or a non-Response return becomes a 500. `entry` is the
// structured tuple from http_next_request: [requestId, method, url, hasBody,
// headers] (headers as [name, value] pairs) — no per-request JSON parse.
async function handleRequest(entry, handler) {
  const requestId = entry[0];
  const method = entry[1];
  const url = entry[2];
  const hasBody = entry[3];
  const headers = entry[4];
  let response;
  try {
    const init = { method, headers };
    // The body streams from the host chunk-by-chunk; nothing is buffered until
    // the handler consumes it. GET/HEAD must not carry a body in the Request
    // constructor (an unread host stream is dropped when the response ends).
    if (hasBody && method !== "GET" && method !== "HEAD") {
      init.body = new ReadableStream({
        async pull(controller) {
          const chunk = await ops.http_body_read(requestId);
          if (chunk === null) controller.close();
          else controller.enqueue(chunk);
        },
      });
    }
    response = await handler(makeServerRequest(url, init));
    if (!(response instanceof Response)) {
      response = new Response(response == null ? "" : String(response));
    }
  } catch {
    response = new Response("Internal Server Error", { status: 500 });
  }

  // Fast path: hand a buffered body to http_respond without an async round-trip.
  // A deferred string body crosses as-is (encoded Rust-side — no utf8_encode op
  // and no intermediate JS byte buffer); already-materialized bytes pass
  // through. Only a streaming body goes through the chunk pump.
  const parts = response._parts();
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
  const args = [requestId, parts.status, out, streamId];
  for (const [name, value] of parts.headers) args.push(name, value);
  // Fire-and-forget: the response is dispatched on this op; not awaiting saves a
  // microtask/tick per request. http_respond only sends on a oneshot (never
  // rejects), so there is no rejection to surface. For a streaming body the
  // status/headers go out now and the chunks flow behind them via the pump.
  ops.http_respond(...args);
  if (stream) await pumpResponseBody(stream, streamId);
}

// The handle returned by serve(): `addr` resolves to the bound address,
// `finished` resolves when the accept loop ends, `stop()` shuts it down.
class Server {
  constructor(hostname, port, handler) {
    let resolveAddr, rejectAddr, resolveFinished;
    this.addr = new Promise((res, rej) => {
      resolveAddr = res;
      rejectAddr = rej;
    });
    this.finished = new Promise((res) => (resolveFinished = res));
    this._id = null;
    this._stopped = false;

    (async () => {
      let info;
      try {
        info = await ops.http_serve(hostname, port);
      } catch (e) {
        rejectAddr(e);
        resolveFinished();
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
          const numHeaders = flat[i++];
          
          const headers = [];
          for (let j = 0; j < numHeaders; j++) {
            headers.push([flat[i++], flat[i++]]);
          }
          
          // Handle each concurrently
          handleRequest([requestId, method, url, hasBody, headers], handler);
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
function serve(options, handler) {
  if (typeof options === "function") {
    handler = options;
    options = {};
  }
  if (typeof handler !== "function") {
    throw new TypeError("serve(options, handler): handler must be a function");
  }
  const { hostname, port } = parseAddress(options);
  return new Server(hostname, port, handler);
}

export { serve };
export default { serve };
