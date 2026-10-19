// A server you can hold in your hand: two byte pipes with a client on one end
// and the test on the other.
//
// This is what the `Transport` seam is for. The handshake, the command pump, the
// subscription routing and the reconnect are all exercised against this — no port
// open, no capability granted (D65) — because the bytes that reach the connection
// are the same whether a socket or this delivered them. The test drives the other
// side: it reads the commands the client sent, and writes back the commands a
// server would. The client-side parsing here is deliberately independent of the
// package's own codec, so a change to the codec cannot make the test agree with
// itself.

const ENC = new TextEncoder();
const DEC = new TextDecoder();

export function concat(a, b) {
  const out = new Uint8Array(a.length + b.length);
  out.set(a, 0);
  out.set(b, a.length);
  return out;
}

function makeChannel() {
  const queue = [];
  let waiter = null;
  let closed = false;
  return {
    push(bytes) {
      if (closed) return;
      if (waiter !== null) {
        const resolve = waiter;
        waiter = null;
        resolve(bytes);
      } else {
        queue.push(bytes);
      }
    },
    end() {
      closed = true;
      if (waiter !== null) {
        const resolve = waiter;
        waiter = null;
        resolve(null);
      }
    },
    read() {
      if (queue.length > 0) return Promise.resolve(queue.shift());
      if (closed) return Promise.resolve(null);
      return new Promise((resolve) => {
        waiter = resolve;
      });
    },
  };
}

function line(bytes, from) {
  for (let i = from; i + 1 < bytes.length; i++) {
    if (bytes[i] === 0x0d && bytes[i + 1] === 0x0a) return i;
  }
  return -1;
}

function count(token) {
  const value = Number(token);
  return Number.isInteger(value) && value >= 0 ? value : null;
}

/** Parses one client command off the front of `buffer`, or `null` if incomplete. */
function tryCommand(buffer) {
  const cr = line(buffer, 0);
  if (cr < 0) return null;
  const text = DEC.decode(buffer.subarray(0, cr));
  const body = cr + 2;
  const fields = text.split(" ");

  if (fields[0] === "CONNECT") {
    const json = text.slice("CONNECT ".length);
    return { consumed: body, command: { kind: "connect", options: JSON.parse(json) } };
  }
  if (fields[0] === "PUB" || fields[0] === "HPUB") {
    const header = fields[0] === "HPUB";
    const sizes = header ? fields.slice(-2) : fields.slice(-1);
    const headerBytes = header ? count(sizes[0]) : 0;
    const totalBytes = count(header ? sizes[1] : sizes[0]);
    if (headerBytes === null || totalBytes === null) throw new Error(`bad PUB sizes: ${text}`);
    const end = body + totalBytes;
    if (buffer.length < end + 2) return null;
    const reply = fields.length === (header ? 5 : 4) ? fields[2] : null;
    return {
      consumed: end + 2,
      command: {
        kind: "pub",
        subject: fields[1],
        reply,
        headers: header ? buffer.subarray(body, body + headerBytes) : null,
        payload: buffer.subarray(body + headerBytes, end),
      },
    };
  }
  if (fields[0] === "SUB") {
    const queue = fields.length === 4 ? fields[2] : undefined;
    const sid = count(fields[fields.length - 1]);
    return { consumed: body, command: { kind: "sub", subject: fields[1], queue, sid } };
  }
  if (fields[0] === "UNSUB") {
    return {
      consumed: body,
      command: {
        kind: "unsub",
        sid: count(fields[1]),
        max: fields.length === 3 ? count(fields[2]) : undefined,
      },
    };
  }
  if (text === "PING") return { consumed: body, command: { kind: "ping" } };
  if (text === "PONG") return { consumed: body, command: { kind: "pong" } };
  throw new Error(`the client sent a line this server does not know: ${text}`);
}

/**
 * A server the connection can talk to, over a `Transport`, with the test holding
 * the other side.
 */
export function fakeBroker() {
  const toServer = makeChannel();
  const toClient = makeChannel();
  let buffer = new Uint8Array(0);
  let tlsUpgrades = 0;

  const transport = {
    read: () => toClient.read(),
    write: async (bytes) => {
      toServer.push(bytes);
    },
    // The in-band upgrade, counted rather than performed: this double carries no
    // bytes through TLS, it only records that the connection asked for it.
    startTls: async () => {
      tlsUpgrades++;
    },
    close: async () => {
      toServer.end();
      toClient.end();
    },
  };

  return {
    transport,
    get tlsUpgrades() {
      return tlsUpgrades;
    },
    /** The next command the client sent, or `null` if it closed the pipe. */
    async next(timeoutMs = 2000) {
      const deadline = Date.now() + timeoutMs;
      for (;;) {
        const parsed = tryCommand(buffer);
        if (parsed !== null) {
          buffer = buffer.subarray(parsed.consumed);
          return parsed.command;
        }
        if (Date.now() > deadline) throw new Error("timed out waiting for a client command");
        const chunk = await toServer.read();
        if (chunk === null) return null;
        buffer = concat(buffer, chunk);
      }
    },
    /** Runs the server's half of the opening handshake and returns the CONNECT. */
    async accept(overrides = {}) {
      this.sendInfo(overrides);
      const connect = await this.next();
      if (connect?.kind !== "connect") throw new Error("the client did not CONNECT");
      const ping = await this.next();
      if (ping?.kind !== "ping") throw new Error("the client did not PING after CONNECT");
      this.sendPong();
      return connect.options;
    },
    /** Sends raw bytes to the client. */
    sendRaw(bytes) {
      toClient.push(bytes);
    },
    sendInfo(overrides = {}) {
      const info = {
        server_id: "TEST",
        version: "2.10.0",
        headers: true,
        max_payload: 1048576,
        ...overrides,
      };
      this.sendRaw(ENC.encode(`INFO ${JSON.stringify(info)}\r\n`));
    },
    sendPing() {
      this.sendRaw(ENC.encode("PING\r\n"));
    },
    sendPong() {
      this.sendRaw(ENC.encode("PONG\r\n"));
    },
    sendErr(text) {
      this.sendRaw(ENC.encode(`-ERR '${text}'\r\n`));
    },
    sendMsg(subject, sid, payload, reply = null) {
      const body = typeof payload === "string" ? ENC.encode(payload) : payload;
      const head =
        reply === null
          ? `MSG ${subject} ${sid} ${body.length}\r\n`
          : `MSG ${subject} ${sid} ${reply} ${body.length}\r\n`;
      this.sendRaw(concat(ENC.encode(head), concat(body, ENC.encode("\r\n"))));
    },
    sendHmsg(subject, sid, headers, payload, reply = null) {
      const headerBytes = typeof headers === "string" ? ENC.encode(headers) : headers;
      const body = typeof payload === "string" ? ENC.encode(payload) : payload;
      const head =
        reply === null
          ? `HMSG ${subject} ${sid} ${headerBytes.length} ${headerBytes.length + body.length}\r\n`
          : `HMSG ${subject} ${sid} ${reply} ${headerBytes.length} ${headerBytes.length + body.length}\r\n`;
      this.sendRaw(concat(ENC.encode(head), concat(headerBytes, concat(body, ENC.encode("\r\n")))));
    },
    /** Ends the pipe, as a server vanishing would. */
    close() {
      toServer.end();
      toClient.end();
    },
  };
}
