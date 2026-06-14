// runtime:net — TCP sockets (SPEC §12). `connect()` follows the WinterTC Sockets
// API: it returns a Socket synchronously whose `.readable`/`.writable` are web
// streams and whose `.opened` resolves once connected. `listen()` returns an
// async-iterable Listener of the same Socket shape. Backed by async ops, gated
// on Net (connect) / NetListen (listen).

const ops = globalThis.__ops;
const encoder = new TextEncoder();

function toBytes(chunk) {
  if (chunk instanceof Uint8Array) return chunk;
  if (typeof chunk === "string") return encoder.encode(chunk);
  if (ArrayBuffer.isView(chunk)) return new Uint8Array(chunk.buffer, chunk.byteOffset, chunk.byteLength);
  if (chunk instanceof ArrayBuffer) return new Uint8Array(chunk);
  throw new TypeError("a socket write expects a string, ArrayBuffer, or ArrayBufferView");
}

// Accepts "host:port" or { hostname | host, port }.
function parseAddress(address) {
  if (address && typeof address === "object") {
    return { hostname: address.hostname ?? address.host ?? "localhost", port: Number(address.port) };
  }
  const s = String(address);
  const i = s.lastIndexOf(":");
  return { hostname: s.slice(0, i) || "localhost", port: Number(s.slice(i + 1)) };
}

// A duplex socket. `conn` is a Promise resolving to { id, remoteAddress, … } —
// the streams await it, so connect() can return synchronously.
class Socket {
  constructor(conn) {
    this._conn = conn;
    this.opened = conn;
    let done;
    this.closed = new Promise((resolve) => (done = resolve));
    this._done = done;
    const self = this;

    this.readable = new ReadableStream({
      async pull(controller) {
        const { id } = await self._conn;
        const chunk = await ops.net_read(id);
        if (chunk === null) {
          controller.close();
          await ops.net_close(id); // peer hung up — drop the socket
          self._finish();
        } else {
          controller.enqueue(chunk);
        }
      },
      cancel() {
        return self.close();
      },
    });

    this.writable = new WritableStream({
      async write(chunk) {
        const { id } = await self._conn;
        await ops.net_write(id, toBytes(chunk));
      },
      // Half-close: send FIN but keep reading the peer's reply.
      async close() {
        const { id } = await self._conn;
        await ops.net_shutdown(id);
      },
      abort() {
        return self.close();
      },
    });
  }

  _finish() {
    if (this._done) {
      this._done();
      this._done = null;
    }
  }

  async close() {
    const { id } = await this._conn;
    await ops.net_close(id);
    this._finish();
  }

  startTls() {
    throw new Error("startTls() is not supported yet");
  }
}

// WinterTC connect(): returns a Socket immediately; .opened settles on connect.
function connect(address, options = {}) {
  const { hostname, port } = parseAddress(address);
  if (options.secureTransport === "starttls") {
    throw new Error("secureTransport 'starttls' is not supported yet");
  }
  const tls = options.secureTransport === "on";
  const conn = ops.net_connect(hostname, port, tls).then((s) => JSON.parse(s));
  return new Socket(conn);
}

// A listening socket: an async iterator of incoming Sockets.
class Listener {
  constructor(ready) {
    this._ready = ready; // Promise<{ id, localAddress, localPort }>
    this.addr = ready.then((s) => ({ hostname: s.localAddress, port: s.localPort }));
  }

  async accept() {
    const { id } = await this._ready;
    const s = await ops.net_accept(id);
    return s === null ? null : new Socket(Promise.resolve(JSON.parse(s)));
  }

  async close() {
    const { id } = await this._ready;
    await ops.net_close_listener(id);
  }

  async *[Symbol.asyncIterator]() {
    for (;;) {
      const socket = await this.accept();
      if (socket === null) return;
      yield socket;
    }
  }
}

function listen(options = {}) {
  const hostname = options.hostname ?? options.host ?? "0.0.0.0";
  const port = Number(options.port) || 0;
  const ready = ops.net_listen(hostname, port).then((s) => JSON.parse(s));
  return new Listener(ready);
}

export { connect, listen };
export default { connect, listen };
