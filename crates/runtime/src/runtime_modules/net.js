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

// Joins a host and port into the WinterTC SocketInfo "host:port" form, bracketing
// an IPv6 host so the port stays unambiguous.
function hostPort(host, port) {
  return host.includes(":") ? `[${host}]:${port}` : `${host}:${port}`;
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
  constructor(conn, { upgrade = null, allowHalfOpen = false } = {}) {
    this._conn = conn;
    // WinterTC SocketInfo: combined "host:port" addresses + the negotiated alpn.
    // remotePort/localPort are kept as a convenience superset.
    this.opened = conn.then((c) => ({
      remoteAddress: hostPort(c.remoteAddress, c.remotePort),
      remotePort: c.remotePort,
      localAddress: hostPort(c.localAddress, c.localPort),
      localPort: c.localPort,
      alpn: c.alpn ?? null,
    }));
    // Set when the socket was opened with secureTransport: "starttls": the
    // { name, alpn } a later startTls() upgrade uses. null ⇒ not upgradable.
    this._upgrade = upgrade;
    // WinterTC allowHalfOpen: when false (default), the peer's FIN tears the
    // whole socket down; when true, the writable stays usable after read EOF.
    this._allowHalfOpen = allowHalfOpen;
    // WinterTC Socket.upgraded — true only after a startTls() upgrade.
    this.upgraded = false;
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
          if (!self._allowHalfOpen) {
            await ops.net_close(id); // peer hung up — drop the whole socket
            self._finish();
          }
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

  // WinterTC startTls(): upgrade a "starttls" socket to TLS, returning a new
  // Socket for the encrypted stream. The original socket is consumed.
  startTls() {
    if (!this._upgrade) {
      throw new TypeError("startTls() requires secureTransport: 'starttls'");
    }
    const { name, alpn } = this._upgrade;
    this._upgrade = null; // single-shot
    const info = this._conn.then(({ id }) => ops.net_start_tls(id, name, alpn));
    const upgraded = new Socket(info, { allowHalfOpen: this._allowHalfOpen });
    upgraded.upgraded = true;
    return upgraded;
  }
}

// WinterTC connect(): returns a Socket immediately; .opened settles on connect.
function connect(address, options = {}) {
  const { hostname, port } = parseAddress(address);
  const mode = options.secureTransport ?? "off";
  if (mode !== "off" && mode !== "on" && mode !== "starttls") {
    throw new TypeError(`invalid secureTransport: ${mode}`);
  }
  const tls = mode === "on";
  // WinterTC SocketOptions: sni (server name override) + alpn (offered protocols;
  // the negotiated one comes back as SocketInfo.alpn). Empty string == no SNI.
  const sni = options.sni == null ? "" : String(options.sni);
  const alpn = Array.isArray(options.alpn) ? options.alpn.map(String) : [];
  const conn = ops.net_connect(hostname, port, tls, sni, alpn);
  // "starttls" opens plaintext now; record the server name (sni, default = host)
  // + alpn a later startTls() will negotiate with.
  const upgrade = mode === "starttls" ? { name: sni || hostname, alpn } : null;
  return new Socket(conn, { upgrade, allowHalfOpen: options.allowHalfOpen === true });
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
    return s === null ? null : new Socket(Promise.resolve(s));
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
  const ready = ops.net_listen(hostname, port);
  return new Listener(ready);
}

export { connect, listen };
export default { connect, listen };
