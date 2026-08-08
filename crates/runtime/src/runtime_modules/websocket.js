// runtime:websocket — the WebSocket *server* side (DECISIONS D29). The client
// is the `WebSocket` global; serving is capability-gated host I/O, so it lives
// in a runtime: module like `runtime:net` `listen()` / `runtime:http` `serve()`.
//
// `serve()` returns a WebSocketServer: an async-iterable of accepted, already-open
// server-side sockets. Each connection runs the same push→pull receive-pump as
// the client global (one ws_recv outstanding, re-armed per frame; D4), so it
// rides the embedder's tick with no owned loop. Binding requires NetListen; the
// per-connection send/recv/close need no capability (the accept authorized them).
// ws: only — a wss: server is a follow-up.

const ops = globalThis.__ops;
// Which host request a server-side Request came from — set by the prelude when
// runtime:http builds one, and `undefined` for every other Request (D55).
const requestIdOf = globalThis.__serverRequestId;
// A 101 the guest cannot construct itself: Fetch bounds `new Response` to
// 200-599, and an informational status is something a server emits.
const upgradeResponse = globalThis.__upgradeResponse;
const encoder = new TextEncoder();

// Connection → host socket id, so broadcast() can batch without exposing the id.
const CONN_ID = new WeakMap();

function toBytes(chunk) {
  if (chunk instanceof Uint8Array) return chunk;
  if (typeof chunk === "string") return encoder.encode(chunk);
  if (ArrayBuffer.isView(chunk)) return new Uint8Array(chunk.buffer, chunk.byteOffset, chunk.byteLength);
  if (chunk instanceof ArrayBuffer) return new Uint8Array(chunk);
  throw new TypeError("a WebSocket send expects a string, Blob, ArrayBuffer, or ArrayBufferView");
}

// An accepted server-side connection. Open from the start (the handshake is done
// before accept resolves), so it has no CONNECTING state — just message/close
// events, send, close, and binaryType, over the shared ws_send/ws_recv/ws_close.
class WebSocketConnection extends EventTarget {
  #id;
  #closed = false;
  #binaryType = "blob";
  #bufferedAmount = 0;
  #handlers = { message: null, close: null, error: null };

  // `id` is a number for a connection the host already has, or a promise for
  // one still being handed over — an HTTP upgrade has no connection until its
  // `101` is on the wire (D55). Held as a promise either way so there is one
  // path, the same shape `runtime:net`'s Socket uses for a dialled connection.
  constructor(id) {
    super();
    this.#id = Promise.resolve(id);
    CONN_ID.set(this, this.#id);
    this.#pump();
  }

  // Bytes handed to send() that the host has not taken yet — the same meaning
  // the WebSocket global's `bufferedAmount` carries, and the only way a sender
  // can feel a peer that has stopped reading: send() is fire-and-forget, so
  // writing faster than the peer reads never stalls this code, it queues.
  //
  // A queue that keeps growing is a peer to stop sending to. Past the server's
  // `maxBufferedAmount` the host closes the connection with 1013 rather than
  // hold more, so ignoring this costs the connection, not the process.
  get bufferedAmount() {
    return this.#bufferedAmount;
  }

  get binaryType() {
    return this.#binaryType;
  }
  set binaryType(value) {
    if (value === "blob" || value === "arraybuffer") this.#binaryType = value;
  }

  get onmessage() {
    return this.#handlers.message;
  }
  set onmessage(fn) {
    this.#setHandler("message", fn);
  }
  get onclose() {
    return this.#handlers.close;
  }
  set onclose(fn) {
    this.#setHandler("close", fn);
  }
  get onerror() {
    return this.#handlers.error;
  }
  set onerror(fn) {
    this.#setHandler("error", fn);
  }

  send(data) {
    if (this.#closed) return;
    // A Blob is read asynchronously, and its bytes count from the moment they
    // are promised: the queue they will join is the thing being measured.
    if (data instanceof Blob) {
      const size = data.size;
      this.#bufferedAmount += size;
      data
        .arrayBuffer()
        .then((buf) => ops.ws_send(this.#id, new Uint8Array(buf)))
        .catch(() => {})
        .finally(() => {
          this.#bufferedAmount -= size;
        });
      return;
    }
    const payload = toBytesOrString(data);
    const size = typeof payload === "string" ? encoder.encode(payload).length : payload.byteLength;
    this.#bufferedAmount += size;
    this.#id
      .then((id) => ops.ws_send(id, payload))
      .catch(() => {})
      .finally(() => {
        this.#bufferedAmount -= size;
      });
  }

  close(code, reason) {
    if (this.#closed) return;
    this.#closed = true;
    const c = code === undefined ? null : code;
    this.#id
      .then((id) => ops.ws_close(id, c, reason === undefined ? "" : String(reason)))
      .catch(() => {});
  }

  #setHandler(name, value) {
    const current = this.#handlers[name];
    if (current) this.removeEventListener(name, current);
    const fn = typeof value === "function" ? value : null;
    this.#handlers[name] = fn;
    if (fn) this.addEventListener(name, fn);
  }

  async #pump() {
    try {
      // A handover that never completed — the client vanished before the `101`
      // reached it — throws here, and is the same abnormal close as a
      // connection that dropped a moment later.
      const id = await this.#id;
      for (;;) {
        const frame = await ops.ws_recv(id);
        if (frame === null) {
          this.#finish(1006, "", false);
          return;
        }
        if (frame.type === "close") {
          this.#finish(frame.code, frame.reason, true);
          return;
        }
        const data =
          frame.type === "text"
            ? frame.data
            : this.#binaryType === "arraybuffer"
              ? frame.data.slice().buffer
              : new Blob([frame.data]);
        this.dispatchEvent(new MessageEvent("message", { data }));
      }
    } catch {
      this.#finish(1006, "", false);
    }
  }

  #finish(code, reason, wasClean) {
    this.#closed = true;
    this.dispatchEvent(new CloseEvent("close", { code, reason, wasClean }));
  }
}

// Keep a text frame as a string (so it arrives as text); everything else → bytes.
function toBytesOrString(data) {
  return typeof data === "string" ? data : toBytes(data);
}

// A listening WebSocket server: an async iterator of incoming connections.
class WebSocketServer {
  constructor(ready) {
    this._ready = ready; // Promise<{ id, hostname, port }>
    this.addr = ready.then((s) => ({ hostname: s.hostname, port: s.port }));
  }

  async accept() {
    const { id } = await this._ready;
    const conn = await ops.ws_accept(id);
    return conn === null ? null : new WebSocketConnection(conn.id);
  }

  async close() {
    const { id } = await this._ready;
    await ops.ws_close_server(id);
  }

  async *[Symbol.asyncIterator]() {
    for (;;) {
      const conn = await this.accept();
      if (conn === null) return;
      yield conn;
    }
  }
}

// When to give up on a connection that has not finished its opening handshake.
// Deliberately the same shape, spelling, and crossing convention as
// `runtime:http`'s `serve({ timeouts })`: `null` on the wire means "the guest
// said nothing" so the host default applies, and an explicit `null` from the
// guest means "off" and crosses as 0. The number itself lives only on the Rust
// side, so the two copies cannot drift.
function parseHandshakeTimeout(options) {
  const t = (options ?? {}).timeouts;
  if (t === undefined || t === null) return null; // host default
  if (typeof t !== "object") {
    throw new TypeError(`serve: timeouts must be an object, got ${typeof t}`);
  }
  const v = t.handshake;
  if (v === undefined) return null; // host default
  if (v === null) return 0; // explicitly disabled
  if (typeof v !== "number") {
    throw new TypeError(`serve: timeouts.handshake must be a number of ms or null`);
  }
  if (!Number.isFinite(v) || v < 0) {
    throw new RangeError("serve: timeouts.handshake must be a finite, non-negative number of ms");
  }
  return v;
}

// The most connections to hold at once. Absent (`null` on the wire) means no
// limit — the deployment's descriptor budget is not something the runtime can
// read, and a cap guessed for you would throttle real traffic with no error
// anywhere to explain it. This matters more here than on an HTTP server: HTTP
// connections churn, while WebSocket connections are long-lived by design, so
// this is what decides whether the count has an upper bound at all.
function parseMaxConnections(options) {
  return count(options, "maxConnections");
}

// The most connections one *peer address* may hold at once. Absent means no
// limit, and for a sharper reason: the count is per address, so everything
// behind one NAT or one load balancer shares a budget — with a proxy in front,
// every connection has the same source and any cap here caps the whole service.
//
// Without it, `maxConnections` bounds what the deployment spends and nothing
// else: one peer opening every slot fills the server exactly as a thousand
// peers opening one each do. A connection over this is *refused*, where one
// over `maxConnections` waits — an excess there is legitimate traffic queueing
// for a slot, and an excess here is one client past its share.
function parseMaxConnectionsPerIp(options) {
  return count(options, "maxConnectionsPerIp");
}

// The most bytes that may sit queued for one connection before the host closes
// it with 1013 ("try again later"). Unlike the connection caps this is *on* by
// default (8 MiB), because the number does not depend on what the deployment
// knows: `send()` is fire-and-forget, so a peer that stops reading a fan-out
// accumulates messages on the host side with nothing bounding the total, and a
// queue that deep is already a peer several messages behind. `0` turns it off.
function parseMaxBufferedAmount(options) {
  const value = (options ?? {}).maxBufferedAmount;
  if (value === undefined || value === null) return null; // host default
  if (typeof value !== "number") {
    throw new TypeError(
      `serve: maxBufferedAmount must be a number of bytes or null, got ${typeof value}`,
    );
  }
  if (!Number.isInteger(value) || value < 0) {
    throw new RangeError("serve: maxBufferedAmount must be a non-negative integer of bytes");
  }
  return value;
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

// Takes over the connection a server request arrived on, turning it into a
// WebSocket (D55) — the shape Deno, Bun and Node all settle on, where a TLS
// endpoint serves `https:` and `wss:` on one port with one certificate.
//
// Returns the 101 the handler must return and the socket it becomes. The order
// is the protocol's: the response goes out first, and only then is there a
// connection to drive — so the socket's id is a promise, exactly as a dialled
// `Socket`'s is in runtime:net, and messages posted before it resolves queue.
//
// The handshake headers are the host's to add: `Sec-WebSocket-Accept` is a
// SHA-1 of a key the guest never sees, and one nobody can get wrong beats one
// everybody has to.
function upgradeWebSocket(request) {
  if (!(request instanceof Request)) {
    throw new TypeError(
      `upgradeWebSocket: expected a Request, got ${request === null ? "null" : typeof request}`,
    );
  }
  const requestId = requestIdOf(request);
  if (requestId === undefined) {
    throw new TypeError(
      "upgradeWebSocket: this Request did not come from a runtime:http handler, so there is " +
        "no connection to take over",
    );
  }
  // Started before the 101 is returned and settles after it: the host resolves
  // this once the response is on the wire. Awaiting it here would deadlock on a
  // response this function has not returned yet.
  const connection = ops.ws_upgrade(requestId);
  return {
    response: upgradeResponse(),
    socket: new WebSocketConnection(connection),
  };
}

function serve(options = {}) {
  const hostname = options.hostname ?? options.host ?? "0.0.0.0";
  const port = Number(options.port) || 0;
  // Validated before the bind, so a bad option is a TypeError at the call
  // rather than a server that is listening with the wrong policy.
  const handshake = parseHandshakeTimeout(options);
  const maxConnections = parseMaxConnections(options);
  const maxConnectionsPerIp = parseMaxConnectionsPerIp(options);
  const maxBufferedAmount = parseMaxBufferedAmount(options);
  const ready = ops.ws_serve(
    hostname,
    port,
    handshake,
    maxConnections,
    maxConnectionsPerIp,
    maxBufferedAmount,
  );
  return new WebSocketServer(ready);
}

// Send one message to many connections in a single host crossing — the batched
// form of calling `.send()` in a loop (one payload marshal, concurrent enqueue,
// no head-of-line blocking on a slow peer). `connections` is any iterable of
// accepted server connections.
//
// A **closed** connection is still passed through: the host holds the live
// socket table and drops ids that are no longer in it, which is the only place
// the question can be answered without a race. Keeping a closed connection in
// the room's set until its close handler removes it is ordinary, so that must
// not be an error.
//
// Anything that is not a connection at all is a different matter, and used to
// be skipped just as quietly: `broadcast([...room, undefined], msg)` sent to the
// rest and said nothing, and a room that had somehow filled with the wrong type
// broadcast to nobody and still returned normally. `CONN_ID` is set in the
// constructor and never removed, so its absence means precisely "this was never
// a connection" — a brand check, not a liveness one — and that is a caller bug
// worth a TypeError. Every element is checked before anything is sent, so a bad
// one fails the whole call rather than half-delivering it.
function broadcast(connections, data) {
  const ids = [];
  let index = 0;
  for (const conn of connections) {
    const id = CONN_ID.get(conn);
    if (id === undefined) {
      throw new TypeError(
        `broadcast(): connections[${index}] is not a WebSocket connection`,
      );
    }
    ids.push(id);
    index += 1;
  }
  if (ids.length === 0) return;
  // The brand check above is synchronous, so a bad element is still a TypeError
  // at the call. The ids are not: one of these connections may still be
  // completing an HTTP upgrade, and a room is not a reason to leave it out.
  const payload = toBytesOrString(data);
  Promise.all(ids)
    .then((resolved) => ops.ws_broadcast(resolved, payload))
    .catch(() => {});
}

export { serve, broadcast, upgradeWebSocket };
export default { serve, broadcast, upgradeWebSocket };
