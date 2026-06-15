// runtime:http — an HTTP/1.1 server: `serve((request) => response)`. The handler
// is called with a web `Request` and returns (or resolves to) a web `Response`
// — the same Fetch API objects `fetch` uses. Backed by async ops over a vetted
// HTTP backend, gated on NetListen (binding the listening socket). Request and
// response bodies are buffered (streaming bodies are a follow-up).

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
    let body = null;
    if (hasBody) {
      // The body is fully buffered host-side; one read drains it.
      body = await ops.http_body_read(requestId);
    }
    const init = { method, headers };
    // GET/HEAD must not carry a body in the Request constructor.
    if (body && method !== "GET" && method !== "HEAD") init.body = body;
    response = await handler(makeServerRequest(url, init));
    if (!(response instanceof Response)) {
      response = new Response(response == null ? "" : String(response));
    }
  } catch {
    response = new Response("Internal Server Error", { status: 500 });
  }

  // Fast path: pull the buffered body bytes synchronously (no async
  // arrayBuffer() round-trip). Streaming bodies fall back to draining async.
  const parts = response._parts();
  let out = parts.bytes;
  if (out === null && parts.stream) {
    const buf = await response.arrayBuffer();
    out = buf.byteLength > 0 ? new Uint8Array(buf) : null;
  }
  const args = [requestId, parts.status, out];
  for (const [name, value] of parts.headers) args.push(name, value);
  // Fire-and-forget: the response is dispatched on this op; not awaiting saves a
  // microtask/tick per request. http_respond only sends on a oneshot (never
  // rejects), so there is no rejection to surface.
  ops.http_respond(...args);
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
        info = JSON.parse(await ops.http_serve(hostname, port));
      } catch (e) {
        rejectAddr(e);
        resolveFinished();
        return;
      }
      this._id = info.id;
      resolveAddr({ hostname: info.localAddress, port: info.localPort });

      while (!this._stopped) {
        const batch = await ops.http_next_request(this._id);
        if (batch === null) break; // server closed
        // A batch of structured request tuples (drained in one crossing). Handle
        // each concurrently: don't await, so one slow handler can't block the
        // accept loop. Errors are swallowed inside handleRequest.
        for (let i = 0; i < batch.length; i++) handleRequest(batch[i], handler);
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
