// runtime:net — TCP sockets and UDP datagrams (SPEC §12). `connect()` follows
// the WinterTC Sockets API: it returns a Socket synchronously whose
// `.readable`/`.writable` are web streams and whose `.opened` resolves once
// connected. `listen()` returns an async-iterable Listener of the same Socket
// shape. `bind()` returns a DatagramSocket — messages, not a byte stream, so it
// has `send`/`receive` rather than streams (DECISIONS D58). Backed by async ops,
// gated on Net (connect, send) / NetListen (listen, bind).

const ops = globalThis.__ops;
const encoder = new TextEncoder();
const EMPTY = new Uint8Array(0);

// WinterTC SocketError: socket failures surface as a TypeError whose message is
// prefixed "SocketError: ". `socketError` builds one; `socketOp` rewraps the
// rejections coming back from the host ops so connection/TLS/I/O failures conform
// too — without double-prefixing an error that already carries the marker. The
// host error's stable `code` (SPEC §6 Phase 13) survives the rewrap.
function socketError(message, code) {
  const err = new TypeError(`SocketError: ${message}`);
  if (code !== undefined) err.code = code;
  return err;
}

function socketOp(promise) {
  return promise.catch((e) => {
    if (e instanceof TypeError && e.message.startsWith("SocketError: ")) throw e;
    throw socketError(e && e.message != null ? e.message : String(e), e?.code);
  });
}

function toBytes(chunk) {
  if (chunk instanceof Uint8Array) return chunk;
  if (typeof chunk === "string") return encoder.encode(chunk);
  if (ArrayBuffer.isView(chunk)) return new Uint8Array(chunk.buffer, chunk.byteOffset, chunk.byteLength);
  if (chunk instanceof ArrayBuffer) return new Uint8Array(chunk);
  throw socketError("a socket write expects a string, ArrayBuffer, or ArrayBufferView");
}

// Joins a host and port into the WinterTC SocketInfo "host:port" form, bracketing
// an IPv6 host so the port stays unambiguous.
function hostPort(host, port) {
  return host.includes(":") ? `[${host}]:${port}` : `${host}:${port}`;
}

// A TCP port, validated. `Number("nope")` is NaN and `Number(-1)` is -1, and
// both used to be handed to the host, where they became port 0 — so a typo'd
// port silently connected somewhere else (or bound an ephemeral port) instead
// of saying so. `connect` needs a real port; `listen` treats 0 as "pick one",
// which is the documented way to get an ephemeral port.
function validPort(value, { allowZero }) {
  const port = value === undefined || value === "" ? 0 : Number(value);
  if (!Number.isInteger(port) || port < 0 || port > 65535 || (port === 0 && !allowZero)) {
    throw socketError(`invalid port: ${String(value)}`);
  }
  return port;
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
    // Wrap once: every consumer (opened, the streams, close, a later startTls)
    // awaits _conn, so a connect/TLS failure surfaces uniformly as a SocketError.
    this._conn = socketOp(conn);
    // WinterTC SocketInfo: combined "host:port" addresses + the negotiated alpn.
    // remotePort/localPort are kept as a convenience superset.
    this.opened = this._conn.then((c) => ({
      remoteAddress: hostPort(c.remoteAddress, c.remotePort),
      remotePort: c.remotePort,
      localAddress: hostPort(c.localAddress, c.localPort),
      localPort: c.localPort,
      alpn: c.alpn ?? null,
    }));
    // One connect failure rejects several surfaces — `opened`, the streams,
    // close(), a later startTls() — because all of them derive from `_conn`.
    // `opened` is built eagerly here, so a program that handled the failure on
    // one of the others (reading `readable` is the common shape) still had this
    // one left over, and an unhandled rejection took the whole process down: a
    // single unreachable host could stop a server that had already dealt with
    // it. Marking it handled drops the *duplicate* report, not the error — a
    // later `await socket.opened` still rejects, since this catch settles its
    // own derived promise and leaves `opened` untouched.
    this.opened.catch(() => {});
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
        if (self._consumed) return controller.close();
        const { id } = await self._conn;
        const chunk = await socketOp(ops.net_read(id));
        if (chunk === null) {
          controller.close();
          if (!self._allowHalfOpen) {
            await self.close(); // peer hung up — drop the whole socket
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
        await socketOp(ops.net_write(id, toBytes(chunk)));
      },
      // Half-close: send FIN but keep reading the peer's reply. Nothing to
      // half-close once the whole socket has gone — the peer's FIN on a socket
      // that disallows half-open closes it from under this stream, and the
      // writable is ended afterwards either way.
      async close() {
        if (self._closing || self._consumed) return;
        const { id } = await self._conn;
        await socketOp(ops.net_shutdown(id));
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

  // WinterTC close(reason?): the optional reason is advisory — accepted for
  // signature compatibility; the transport just tears the connection down.
  //
  // Closed once, however many ways it is reached: an explicit close(), the
  // readable's cancel(), the writable's abort(), and read EOF on a socket that
  // does not allow half-open all arrive here. The second one must not reach the
  // host — the id is given back on the first, and naming it again is a request
  // to act on a socket this agent no longer has.
  close(_reason) {
    // A consumed socket has already handed its connection to the upgraded one,
    // which is what closes it.
    if (this._consumed) return Promise.resolve();
    this._closing ??= (async () => {
      const { id } = await this._conn;
      await socketOp(ops.net_close(id));
      this._finish();
    })();
    return this._closing;
  }

  // WinterTC startTls(): upgrade a "starttls" socket to TLS, returning a new
  // Socket for the encrypted stream. The original socket is consumed.
  startTls() {
    if (!this._upgrade) {
      throw socketError("startTls() requires secureTransport: 'starttls'");
    }
    const { name, alpn, ca } = this._upgrade;
    this._upgrade = null; // single-shot
    // The upgrade retires this socket's id and mints another, so from here the
    // original names nothing. Marked rather than left to fail: its own close()
    // in a `finally` is ordinary code, and `ERR_FOREIGN_HANDLE` is the answer
    // to naming another agent's socket, not to naming your own after it was
    // consumed.
    this._consumed = true;
    const info = this._conn.then(({ id }) => ops.net_start_tls(id, name, alpn, ca));
    const upgraded = new Socket(info, { allowHalfOpen: this._allowHalfOpen });
    upgraded.upgraded = true;
    return upgraded;
  }
}

// WinterTC connect(): returns a Socket immediately; .opened settles on connect.
function connect(address, options = {}) {
  const parsed = parseAddress(address);
  const hostname = parsed.hostname;
  const port = validPort(parsed.port, { allowZero: false });
  const mode = options.secureTransport ?? "off";
  if (mode !== "off" && mode !== "on" && mode !== "starttls") {
    throw socketError(`invalid secureTransport: ${mode}`);
  }
  const tls = mode === "on";
  // WinterTC SocketOptions: sni (server name override) + alpn (offered protocols;
  // the negotiated one comes back as SocketInfo.alpn). Empty string == no SNI.
  const sni = options.sni == null ? "" : String(options.sni);
  const alpn = Array.isArray(options.alpn) ? options.alpn.map(String) : [];
  // `ca`: extra trust anchors, as PEM. Added to the built-in roots rather than
  // replacing them, so naming a private authority does not quietly stop the
  // program trusting every public one — and it can only make verification
  // accept more certificates, never skip it. A private CA is the ordinary case
  // for an internal database or service mesh, and without this there is no way
  // to reach one at all.
  const ca = options.ca == null ? EMPTY : toBytes(options.ca);
  const conn = ops.net_connect(hostname, port, tls, sni, alpn, ca);
  // "starttls" opens plaintext now; record the server name (sni, default = host)
  // + alpn + ca a later startTls() will negotiate with.
  const upgrade = mode === "starttls" ? { name: sni || hostname, alpn, ca } : null;
  return new Socket(conn, { upgrade, allowHalfOpen: options.allowHalfOpen === true });
}

// A listening socket: an async iterator of incoming Sockets.
class Listener {
  constructor(ready) {
    this._ready = ready; // Promise<{ id, localAddress, localPort }>
    this.addr = ready.then((s) => ({ hostname: s.localAddress, port: s.localPort }));
    // Same shape as Socket.opened above: `addr` is built eagerly, so a bind
    // that fails (a denied address, a port already in use) rejects it whether
    // or not anyone asked for the address. A caller that handles the failure on
    // accept() has already reported it; this stops the leftover from ending the
    // process. `await listener.addr` still rejects.
    this.addr.catch(() => {});
  }

  async accept() {
    // A closed listener accepts nothing — the answer the async iterator reads
    // as "stop", rather than an error from a call that had no business being
    // made.
    if (this._closed) return null;
    const { id } = await this._ready;
    const s = await ops.net_accept(id);
    return s === null ? null : new Socket(Promise.resolve(s));
  }

  close() {
    this._closed ??= (async () => {
      const { id } = await this._ready;
      await ops.net_close_listener(id);
    })();
    return this._closed;
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
  const port = validPort(options.port, { allowZero: true });
  // secureTransport: "on" terminates TLS — every accepted Socket is encrypted.
  // It needs a cert + key (PEM string or bytes); alpn advertises protocols, the
  // negotiated one comes back as the accepted Socket's SocketInfo.alpn.
  const mode = options.secureTransport ?? "off";
  if (mode !== "off" && mode !== "on") {
    throw socketError(`invalid secureTransport: ${mode}`);
  }
  let cert = EMPTY, key = EMPTY;
  const alpn = Array.isArray(options.alpn) ? options.alpn.map(String) : [];
  if (mode === "on") {
    if (options.cert == null || options.key == null) {
      throw socketError("secureTransport: 'on' requires cert and key");
    }
    cert = toBytes(options.cert);
    key = toBytes(options.key);
  }
  // Wrapped like `Socket._conn`: a bind failure (a denied address, a port in
  // use, a privileged port) is a socket failure and carries the documented
  // `TypeError: SocketError: …` shape, rather than the raw host `Error` that
  // made `listen` report differently from `connect` for the same class of
  // problem.
  // `SO_REUSEPORT`: let several processes bind this same port and have the
  // kernel balance connections across them — how a server is run across cores
  // without a front proxy, and how one is replaced without dropping
  // connections. Unix-only; the host refuses it elsewhere rather than binding
  // exclusively and leaving the caller to find out when a second process
  // cannot start.
  const reusePort = options.reusePort ?? false;
  if (typeof reusePort !== "boolean") {
    throw socketError(`reusePort must be a boolean, got ${typeof reusePort}`);
  }
  const ready = socketOp(ops.net_listen(hostname, port, cert, key, alpn, reusePort));
  return new Listener(ready);
}

// A bound UDP socket. Not a stream: UDP delivers whole messages from whoever
// sent them, so a datagram carries its own sender and `receive()` hands back one
// message at a time. Building a ReadableStream over that would have to invent a
// queue in front of the kernel's, and would lose the message boundary that is
// the only thing UDP guarantees.
class DatagramSocket {
  constructor(ready) {
    // Wrapped once, like Socket._conn: a failed bind must reach `addr`, a
    // `send`, a `receive` and `close()` as the same SocketError.
    this._ready = socketOp(ready);
    this.addr = this._ready.then((s) => ({ hostname: s.localAddress, port: s.localPort }));
    // Same reasoning as Listener.addr: built eagerly, so a bind that fails
    // rejects it whether or not anyone asked for the address, and a program
    // that handled the failure elsewhere is not ended by the leftover.
    this.addr.catch(() => {});
    let done;
    this.closed = new Promise((resolve) => (done = resolve));
    this._done = done;
  }

  // Sends one datagram, resolving with the number of bytes sent. `address` is
  // required unless the socket is connected, where it is the peer's.
  async send(data, address) {
    if (this._closing) throw socketError("the socket is closed");
    const bytes = toBytes(data);
    let hostname = "";
    let port = 0;
    if (address !== undefined && address !== null) {
      const parsed = parseAddress(address);
      hostname = parsed.hostname;
      port = validPort(parsed.port, { allowZero: false });
    }
    const { id } = await this._ready;
    return socketOp(ops.net_datagram_send(id, bytes, hostname, port));
  }

  // The next datagram: `{ data, address, port }`, or null once closed. One call
  // is one message — including a zero-length one, which is a message and not an
  // end of stream.
  async receive() {
    if (this._closing) return null;
    const { id } = await this._ready;
    return socketOp(ops.net_datagram_receive(id));
  }

  // Fixes the peer: later sends need no address, and datagrams from anyone else
  // are discarded. Nothing is sent — UDP has no handshake — so this resolves
  // against a host that may not be listening at all.
  async connect(address) {
    const parsed = parseAddress(address);
    const port = validPort(parsed.port, { allowZero: false });
    const { id } = await this._ready;
    const info = await socketOp(ops.net_datagram_connect(id, parsed.hostname, port));
    return {
      remoteAddress: hostPort(info.remoteAddress, info.remotePort),
      remotePort: info.remotePort,
      localAddress: hostPort(info.localAddress, info.localPort),
      localPort: info.localPort,
    };
  }

  // Multicast membership. `interface` names which local interface carries it —
  // an IPv4 address for a v4 group, an interface index for a v6 one — and
  // defaults to the OS's choice, which is only unambiguous on a host with one
  // interface.
  joinMulticast(group, options = {}) {
    return this._multicast(group, options, true);
  }

  leaveMulticast(group, options = {}) {
    return this._multicast(group, options, false);
  }

  async _multicast(group, options, join) {
    if (typeof group !== "string" || group === "") {
      throw socketError("a multicast group is required");
    }
    const iface = options?.interface == null ? "" : String(options.interface);
    const { id } = await this._ready;
    return socketOp(ops.net_datagram_multicast(id, group, iface, join));
  }

  // Closed once, however many ways it is reached — the id is given back on the
  // first, and naming it again is a request to act on a socket this agent no
  // longer has. A parked `receive()` resolves to null rather than waiting for a
  // datagram that can no longer arrive.
  close() {
    this._closing ??= (async () => {
      const { id } = await this._ready;
      await socketOp(ops.net_datagram_close(id));
      if (this._done) {
        this._done();
        this._done = null;
      }
    })();
    return this._closing;
  }

  async *[Symbol.asyncIterator]() {
    for (;;) {
      const datagram = await this.receive();
      if (datagram === null) return;
      yield datagram;
    }
  }
}

// An integer socket option in an inclusive range, or undefined when the guest
// said nothing — which leaves the OS default in place rather than substituting
// a number chosen here.
function optionalInt(value, name, max) {
  if (value == null) return undefined;
  const n = Number(value);
  if (!Number.isInteger(n) || n < 0 || n > max) {
    throw socketError(`invalid ${name}: ${String(value)}`);
  }
  return n;
}

function boolOption(value, name) {
  if (value === undefined) return false;
  if (typeof value !== "boolean") {
    throw socketError(`${name} must be a boolean, got ${typeof value}`);
  }
  return value;
}

// Binds a UDP socket (capability: NetListen — this takes a port, and a port is
// how a process is reached). Sending needs Net as well, checked per datagram.
function bind(options = {}) {
  const hostname = options.hostname ?? options.host ?? "0.0.0.0";
  const port = validPort(options.port, { allowZero: true });
  const multicastLoopback =
    options.multicastLoopback === undefined
      ? null
      : boolOption(options.multicastLoopback, "multicastLoopback");
  const ready = ops.net_bind_datagram(
    hostname,
    port,
    boolOption(options.reusePort, "reusePort"),
    boolOption(options.reuseAddress, "reuseAddress"),
    boolOption(options.broadcast, "broadcast"),
    // 255 is the width of the IPv4 TTL field and of the IPv6 hop limit; a
    // larger number is a mistake, not a longer reach.
    optionalInt(options.ttl, "ttl", 255) ?? null,
    optionalInt(options.multicastTtl, "multicastTtl", 255) ?? null,
    multicastLoopback,
  );
  return new DatagramSocket(ready);
}

export { connect, listen, bind };
export default { connect, listen, bind };
