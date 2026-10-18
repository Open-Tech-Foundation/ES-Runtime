// A broker that is a script rather than a socket, shared by the unit tests.
//
// The connection and the channel are exercised the same way: the client is
// handed an in-memory transport, and this reads what it sent and says what a
// test tells it to. No port is opened and no capability is granted.
import { Connection } from "../../dist/connection.js";
import { encodeFrame, FrameDecoder, FrameType } from "../../dist/protocol/frame.js";
import { decodeMethod, encodeMethod } from "../../dist/protocol/methods.js";
import { encodeContentHeader } from "../../dist/protocol/properties.js";

export const PROTOCOL_HEADER = [0x41, 0x4d, 0x51, 0x50, 0x00, 0x00, 0x09, 0x01];

/** A push/pull queue of byte chunks; `null` is end of stream. */
class Pipe {
  #items = [];
  #waiters = [];
  #ended = false;

  push(bytes) {
    if (this.#ended) return;
    const waiter = this.#waiters.shift();
    if (waiter !== undefined) waiter(bytes);
    else this.#items.push(bytes);
  }

  end() {
    this.#ended = true;
    for (const waiter of this.#waiters.splice(0)) waiter(null);
  }

  next() {
    const item = this.#items.shift();
    if (item !== undefined) return Promise.resolve(item);
    if (this.#ended) return Promise.resolve(null);
    return new Promise((resolve) => this.#waiters.push(resolve));
  }
}

/** Two transports, each writing into the other's stream. */
export function loopback() {
  const toServer = new Pipe();
  const toClient = new Pipe();
  return {
    client: {
      read: () => toClient.next(),
      write: async (bytes) => toServer.push(bytes),
      close: async () => {
        toClient.end();
      },
    },
    server: {
      read: () => toServer.next(),
      write: async (bytes) => toClient.push(bytes),
      close: async () => toClient.end(),
    },
  };
}

/** The other end: reads what the client sent, and says what a test tells it to. */
export class Broker {
  #transport;
  #decoder = new FrameDecoder();
  #pending = [];
  #methods = new Map(); // name -> array of methods
  #any = [];
  #ended = false;

  constructor(transport) {
    this.#transport = transport;
    void this.#pump();
  }

  /** What the client sent as its protocol header, once it has. */
  header = null;
  /** Every frame the client sent, in order. */
  frames = [];

  async #pump() {
    try {
      for (;;) {
        const chunk = await this.#transport.read();
        if (chunk === null) {
          this.#ended = true;
          return;
        }
        for (const byte of chunk) this.#pending.push(byte);
        this.#consume();
      }
    } catch {
      this.#ended = true;
    }
  }

  #consume() {
    if (this.header === null) {
      if (this.#pending.length < PROTOCOL_HEADER.length) return;
      this.header = this.#pending.splice(0, PROTOCOL_HEADER.length);
    }
    if (this.#pending.length > 0) {
      this.#decoder.feed(Uint8Array.from(this.#pending));
      this.#pending = [];
    }
    for (let frame = this.#decoder.next(); frame !== null; frame = this.#decoder.next()) {
      this.frames.push(frame);
      if (frame.type !== FrameType.method) continue;
      const method = decodeMethod(frame.payload);
      this.#record(method, frame.channel);
    }
  }

  #record(method, channel) {
    const bucket = this.#methods.get(method.name) ?? [];
    bucket.push({ ...method, channel });
    this.#methods.set(method.name, bucket);
    this.#any.push({ ...method, channel });
  }

  /** Waits for the protocol header, for cases where no method is expected first. */
  async awaitHeader(timeout = 2000) {
    const deadline = Date.now() + timeout;
    while (this.header === null) {
      if (Date.now() > deadline) throw new Error("the client never sent a protocol header");
      await new Promise((resolve) => setTimeout(resolve, 1));
    }
    return this.header;
  }

  /** Waits for the client to send `name`, and hands that method back. */
  async expect(name, timeout = 2000) {
    const deadline = Date.now() + timeout;
    for (;;) {
      const bucket = this.#methods.get(name);
      if (bucket !== undefined && bucket.length > 0) return bucket.shift();
      if (Date.now() > deadline) throw new Error(`the client never sent ${name}`);
      if (this.#ended) throw new Error(`the client closed before ${name}`);
      await new Promise((resolve) => setTimeout(resolve, 1));
    }
  }

  /** Waits for any method, and hands it back. */
  async any(timeout = 2000) {
    const deadline = Date.now() + timeout;
    for (;;) {
      const next = this.#any.shift();
      if (next !== undefined) return next;
      if (Date.now() > deadline) throw new Error("the client sent nothing");
      await new Promise((resolve) => setTimeout(resolve, 1));
    }
  }

  /** Waits for a frame of `type` on `channel`. */
  async expectFrame(type, channel, timeout = 2000) {
    const deadline = Date.now() + timeout;
    for (;;) {
      const at = this.frames.findIndex((frame) => frame.type === type && frame.channel === channel);
      if (at !== -1) return this.frames.splice(at, 1)[0];
      if (Date.now() > deadline) throw new Error(`the client never sent frame type ${type}`);
      await new Promise((resolve) => setTimeout(resolve, 1));
    }
  }

  /** Sends a method to the client. */
  async send(name, args = {}, channel = 0) {
    await this.#transport.write(
      encodeFrame({ type: FrameType.method, channel, payload: encodeMethod(name, args) }),
    );
  }

  /** Sends raw bytes, for the cases that are not a frame at all. */
  async sendRaw(bytes) {
    await this.#transport.write(Uint8Array.from(bytes));
  }

  /** Sends a content header frame. */
  async sendHeader(payload, channel) {
    await this.#transport.write(encodeFrame({ type: FrameType.header, channel, payload }));
  }

  /** Sends a body frame. */
  async sendBody(bytes, channel) {
    await this.#transport.write(
      encodeFrame({ type: FrameType.body, channel, payload: Uint8Array.from(bytes) }),
    );
  }

  end() {
    this.#transport.close();
  }
}

export function bytesEqual(actual, expected) {
  return (
    actual !== null &&
    actual.length === expected.length &&
    expected.every((b, i) => actual[i] === b)
  );
}

/**
 * A broker that completes the handshake, up to and including `open-ok`.
 *
 * A real broker says nothing until it has read the client's protocol header, so
 * this waits for it too. Sending `connection.start` any earlier works when the
 * client has already begun reading, but not when the handshake is started before
 * `open()` — a client that buffers the frame before it has registered its waiter
 * for `connection.start` drops it, which is exactly what a socket never does.
 */
export async function handshake(broker, tune = { channelMax: 0, frameMax: 0, heartbeat: 0 }) {
  await broker.awaitHeader();
  await broker.send("connection.start", {
    versionMajor: 0,
    versionMinor: 9,
    serverProperties: new Map([
      ["product", { type: "string", value: "RabbitMQ" }],
      ["version", { type: "string", value: "4.0.0" }],
    ]),
    mechanisms: "PLAIN AMQPLAIN",
    locales: "en_US",
  });
  const startOk = await broker.expect("connection.start-ok");
  await broker.send("connection.tune", tune);
  await broker.expect("connection.tune-ok");
  await broker.expect("connection.open");
  await broker.send("connection.open-ok");
  return startOk;
}

/** Closes a connection the way a real broker would: answering its `close`. */
export async function shut(connection, broker) {
  if (!connection.usable) return;
  const closing = connection.close();
  await broker.expect("connection.close");
  await broker.send("connection.close-ok");
  await closing;
}

/** Opens a connection and one channel over it, and returns both ends. */
export async function openChannel(tune) {
  const pair = loopback();
  const broker = new Broker(pair.server);
  const connection = new Connection(pair.client, { heartbeat: 0 });
  const opening = connection.open();
  await handshake(broker, tune);
  await opening;
  const opening2 = connection.channel();
  const channelOpen = await broker.expect("channel.open");
  await broker.send("channel.open-ok", {}, channelOpen.channel);
  const channel = await opening2;
  return { broker, connection, channel, number: channelOpen.channel };
}

/** Sends a content-bearing method as a real broker would: method, header, body. */
export async function sendContent(broker, name, args, channel, properties, body) {
  await broker.send(name, args, channel);
  await broker.sendHeader(
    encodeContentHeader({ classId: 60, weight: 0, bodySize: BigInt(body.length), properties }),
    channel,
  );
  await broker.sendBody(body, channel);
}
