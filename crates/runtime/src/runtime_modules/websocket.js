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
const encoder = new TextEncoder();

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
  #handlers = { message: null, close: null, error: null };

  constructor(id) {
    super();
    this.#id = id;
    this.#pump();
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
    if (data instanceof Blob) {
      data
        .arrayBuffer()
        .then((buf) => ops.ws_send(this.#id, new Uint8Array(buf)))
        .catch(() => {});
      return;
    }
    Promise.resolve(ops.ws_send(this.#id, toBytesOrString(data))).catch(() => {});
  }

  close(code, reason) {
    if (this.#closed) return;
    this.#closed = true;
    const c = code === undefined ? null : code;
    Promise.resolve(
      ops.ws_close(this.#id, c, reason === undefined ? "" : String(reason)),
    ).catch(() => {});
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
      for (;;) {
        const frame = await ops.ws_recv(this.#id);
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

function serve(options = {}) {
  const hostname = options.hostname ?? options.host ?? "0.0.0.0";
  const port = Number(options.port) || 0;
  const ready = ops.ws_serve(hostname, port);
  return new WebSocketServer(ready);
}

export { serve };
export default { serve };
