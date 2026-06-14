// runtime:http — an HTTP/1.1 server: `serve((request) => response)`. The handler
// is called with a web `Request` and returns (or resolves to) a web `Response`
// — the same Fetch API objects `fetch` uses. Backed by async ops over a vetted
// HTTP backend, gated on NetListen (binding the listening socket). Request and
// response bodies are buffered (streaming bodies are a follow-up).

const ops = globalThis.__ops;

function parseAddress(options) {
  const o = options ?? {};
  return {
    hostname: o.hostname ?? o.host ?? "0.0.0.0",
    port: Number(o.port) || 0,
  };
}

// Runs one request through the handler and writes the response back. Never
// throws: a handler error or a non-Response return becomes a 500.
async function handleRequest(meta, handler) {
  const { requestId } = meta;
  let response;
  try {
    let body = null;
    if (meta.hasBody) {
      // The body is fully buffered host-side; one read drains it.
      body = await ops.http_body_read(requestId);
    }
    const init = { method: meta.method, headers: meta.headers };
    // GET/HEAD must not carry a body in the Request constructor.
    if (body && meta.method !== "GET" && meta.method !== "HEAD") init.body = body;
    response = await handler(new Request(meta.url, init));
    if (!(response instanceof Response)) {
      response = new Response(response == null ? "" : String(response));
    }
  } catch {
    response = new Response("Internal Server Error", { status: 500 });
  }

  const buf = await response.arrayBuffer();
  const out = buf.byteLength > 0 ? new Uint8Array(buf) : null;
  const args = [requestId, response.status, out];
  for (const [name, value] of response.headers) args.push(name, value);
  await ops.http_respond(...args);
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
        const metaJson = await ops.http_next_request(this._id);
        if (metaJson === null) break; // server closed
        // Handle concurrently: don't await, so one slow handler can't block the
        // accept loop. Errors are swallowed inside handleRequest.
        handleRequest(JSON.parse(metaJson), handler);
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
