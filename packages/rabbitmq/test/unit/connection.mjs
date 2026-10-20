// The connection, against a broker that is a script rather than a socket.
//
// No port is opened and no capability is granted: the connection is handed an
// in-memory transport, which is the whole reason the transport is a seam. What
// is worth pinning here is the handshake script, the tuning arithmetic, the
// routing of frames to channels, and the refusal paths — none of which a live
// broker will produce on demand.
import { exit } from "runtime:process";
import { Connection, DEFAULT_HEARTBEAT } from "../../dist/connection.js";
import { is, ok, report } from "./assert.mjs";
import { Broker, bytesEqual, handshake, loopback, PROTOCOL_HEADER, shut } from "./broker.mjs";

/** A handler that records what it is told, for the routing tests. */
function recorder() {
  return {
    methods: [],
    headers: [],
    bodies: [],
    closed: [],
    onMethod(channel, method) {
      this.methods.push({ channel, name: method.name, args: method.args });
    },
    onHeader(channel, header) {
      this.headers.push({ channel, header });
    },
    onBody(channel, bytes) {
      this.bodies.push({ channel, bytes });
    },
    onClosed(channel, error) {
      this.closed.push({ channel, error });
    },
  };
}

// -- the handshake ----------------------------------------------------------

{
  const pair = loopback();
  const broker = new Broker(pair.server);
  const connection = new Connection(pair.client, { username: "u", password: "p", vhost: "/vh" });
  const opening = connection.open();

  await broker.send("connection.start", {
    versionMajor: 0,
    versionMinor: 9,
    serverProperties: new Map([["product", { type: "string", value: "RabbitMQ" }]]),
    mechanisms: "PLAIN AMQPLAIN",
    locales: "en_US",
  });
  await broker.expect("connection.start-ok");
  ok(bytesEqual(broker.header, PROTOCOL_HEADER), "the client opens with the AMQP 0-9-1 header");
  ok(broker.frames[0].channel === 0, "the handshake is on channel 0");
  await broker.send("connection.tune", { channelMax: 0, frameMax: 0, heartbeat: 0 });
  const tuneOk = await broker.expect("connection.tune-ok");
  const open = await broker.expect("connection.open");
  await broker.send("connection.open-ok");

  const info = await opening;
  ok(info.version.major === 0 && info.version.minor === 9, "the version is read back");
  ok(info.mechanisms.includes("PLAIN"), "the mechanisms are read back");
  ok(connection.usable, "the connection is usable");
  is(open.args.virtualHost, "/vh", "the virtual host is what was asked for");
  is(tuneOk.args.heartbeat, 60, "the client asks for the default heartbeat");
  ok(
    connection.server.properties.get("product").value === "RabbitMQ",
    "the server properties are kept",
  );
  await shut(connection, broker);
}

{
  const pair = loopback();
  const broker = new Broker(pair.server);
  const connection = new Connection(pair.client, { username: "u", password: "p" });
  const opening = connection.open();
  const startOk = await handshake(broker, { channelMax: 100, frameMax: 65536, heartbeat: 5 });
  await opening;

  is(startOk.args.mechanism, "PLAIN", "PLAIN is the mechanism offered");
  is(startOk.args.response, "\u0000u\u0000p", "the credentials are SASL PLAIN");
  ok(
    startOk.args.clientProperties.get("product").value === "esrun-rabbitmq",
    "the client identifies itself",
  );
  const capabilities = startOk.args.clientProperties.get("capabilities").value;
  ok(capabilities.get("publisher_confirms").value === true, "publisher confirms are declared");
  is(connection.channelMax, 100, "the broker's channel limit is accepted");
  is(connection.frameMax, 65536, "the broker's frame limit is accepted");
  is(connection.heartbeat, 5, "the broker's heartbeat is accepted");
  await shut(connection, broker);
}

// -- the tuning arithmetic --------------------------------------------------

// A zero on either side means "no proposal", so the other value stands; and
// frame-max may not be agreed below the protocol's 4096-octet floor.
const TUNING = [
  [{ heartbeat: 0 }, { heartbeat: 0 }, 0, "both zero disables heartbeats"],
  [{ heartbeat: 10 }, { heartbeat: 0 }, 10, "a broker that proposes none takes ours"],
  [{ heartbeat: 10 }, { heartbeat: 4 }, 4, "the smaller heartbeat wins"],
  [{ heartbeat: 0 }, { heartbeat: 30 }, 30, "a broker that proposes one keeps it"],
  [{}, { heartbeat: 0 }, DEFAULT_HEARTBEAT, "the default is asked for"],
  [{ frameMax: 0 }, { frameMax: 0 }, 131072, "an unlimited frame is capped by our default"],
  [
    { frameMax: 8192 },
    { frameMax: 0 },
    8192,
    "our frame limit is used when the broker proposes none",
  ],
  [{ frameMax: 0 }, { frameMax: 1024 }, 4096, "the agreed frame never drops below 4096"],
  [{ frameMax: 4096 }, { frameMax: 65536 }, 4096, "the smaller frame limit wins"],
];

for (const [settings, tune, expected, what] of TUNING) {
  const pair = loopback();
  const broker = new Broker(pair.server);
  const connection = new Connection(pair.client, settings);
  const opening = connection.open();
  await handshake(broker, { channelMax: 0, frameMax: 0, heartbeat: 0, ...tune });
  await opening;
  const actual = settings.frameMax === undefined ? connection.heartbeat : connection.frameMax;
  is(actual, expected, what);
  await shut(connection, broker);
}

// -- channels ---------------------------------------------------------------

{
  const pair = loopback();
  const broker = new Broker(pair.server);
  const connection = new Connection(pair.client, { heartbeat: 0 });
  const opening = connection.open();
  await handshake(broker);
  await opening;

  const handler = recorder();
  const opened = connection.openChannel(handler);
  const channelOpen = await broker.expect("channel.open");
  is(channelOpen.channel, 1, "the first channel is 1");
  await broker.send("channel.open-ok", {}, 1);
  is(await opened, 1, "the channel number is handed back");

  // An unsolicited method goes to the channel's handler.
  await broker.send("basic.cancel", { consumerTag: "ctag", noWait: false }, 1);
  await new Promise((resolve) => setTimeout(resolve, 5));
  is(handler.methods.length, 1, "an unsolicited method reaches the handler");
  is(handler.methods[0].name, "basic.cancel", "and it is the one that was sent");

  // Content: the header and bodies go to the handler too.
  await broker.send(
    "basic.deliver",
    { consumerTag: "ctag", deliveryTag: 7n, redelivered: false, exchange: "e", routingKey: "r" },
    1,
  );
  await broker.sendHeader(
    Uint8Array.from([
      0x00, 0x3c, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    ]),
    1,
  );
  await broker.sendBody([1, 2, 3], 1);
  await new Promise((resolve) => setTimeout(resolve, 5));
  is(handler.headers.length, 1, "the content header reaches the handler");
  ok(handler.headers[0].header.bodySize === 0n, "and its body size is read");
  is(handler.bodies.length, 1, "the body reaches the handler");

  // A second channel gets the next number, and both coexist.
  const second = connection.openChannel(recorder());
  await broker.expect("channel.open");
  await broker.send("channel.open-ok", {}, 2);
  is(await second, 2, "the next channel is 2");
  ok(connection.hasChannel(1) && connection.hasChannel(2), "both channels are open");

  await shut(connection, broker);
}

{
  // A channel the broker closes is soft: the handler is told, with the reason.
  const pair = loopback();
  const broker = new Broker(pair.server);
  const connection = new Connection(pair.client, { heartbeat: 0 });
  const opening = connection.open();
  await handshake(broker);
  await opening;

  const handler = recorder();
  const opened = connection.openChannel(handler);
  await broker.expect("channel.open");
  await broker.send("channel.open-ok", {}, 1);
  is(await opened, 1, "the channel is open");

  await broker.send(
    "channel.close",
    { replyCode: 404, replyText: "NOT_FOUND - no queue", classId: 50, methodId: 10 },
    1,
  );
  await broker.expect("channel.close-ok");
  await new Promise((resolve) => setTimeout(resolve, 5));
  is(handler.closed.length, 1, "the handler is told the channel closed");
  is(handler.closed[0].error.replyCode, 404, "with the broker's reply code");
  ok(handler.closed[0].error.soft, "and it is a soft error");
  ok(!connection.hasChannel(1), "the channel is gone");
  ok(connection.usable, "and the connection survives");

  await shut(connection, broker);
}

// Closing a channel ourselves asks and waits.
{
  const pair = loopback();
  const broker = new Broker(pair.server);
  const connection = new Connection(pair.client, { heartbeat: 0 });
  const opening = connection.open();
  await handshake(broker);
  await opening;

  const handler = recorder();
  const opened = connection.openChannel(handler);
  await broker.expect("channel.open");
  await broker.send("channel.open-ok", {}, 1);
  await opened;

  const closing = connection.closeChannel(1);
  const close = await broker.expect("channel.close");
  is(close.args.replyCode, 200, "a deliberate close says 200");
  await broker.send("channel.close-ok", {}, 1);
  await closing;
  ok(!connection.hasChannel(1), "the channel is gone");
  is(handler.closed.length, 1, "and the handler was told");
  ok(handler.closed[0].error === null, "with no error");
  await shut(connection, broker);
}

// -- refusal paths ----------------------------------------------------------

{
  // Bad credentials: the broker refuses at `connection.open` with 403.
  const pair = loopback();
  const broker = new Broker(pair.server);
  const connection = new Connection(pair.client, { username: "bad", password: "bad" });
  const opening = connection.open();
  await broker.send("connection.start", {
    versionMajor: 0,
    versionMinor: 9,
    serverProperties: new Map(),
    mechanisms: "PLAIN",
    locales: "en_US",
  });
  await broker.expect("connection.start-ok");
  await broker.send("connection.tune", { channelMax: 0, frameMax: 0, heartbeat: 0 });
  await broker.expect("connection.tune-ok");
  await broker.expect("connection.open");
  await broker.send("connection.close", {
    replyCode: 403,
    replyText: "ACCESS_REFUSED - Login was refused",
    classId: 0,
    methodId: 0,
  });

  let failure = null;
  try {
    await opening;
  } catch (e) {
    failure = e;
  }
  is(failure?.code, "ERR_RABBITMQ_AUTH", "a refused login is an auth failure");
  is(failure?.replyCode, 403, "with the broker's code");
  ok(!connection.usable, "and the connection is not usable");
}

{
  // A password RabbitMQ refuses is refused at the SASL step: the broker closes
  // the connection in place of `connection.tune`, before it ever tunes.
  const pair = loopback();
  const broker = new Broker(pair.server);
  const connection = new Connection(pair.client, { username: "bad", password: "bad" });
  const opening = connection.open();
  await broker.send("connection.start", {
    versionMajor: 0,
    versionMinor: 9,
    serverProperties: new Map(),
    mechanisms: "PLAIN",
    locales: "en_US",
  });
  await broker.expect("connection.start-ok");
  await broker.send("connection.close", {
    replyCode: 403,
    replyText: "ACCESS_REFUSED - Login was refused using authentication mechanism PLAIN",
    classId: 0,
    methodId: 0,
  });

  let failure = null;
  try {
    await opening;
  } catch (e) {
    failure = e;
  }
  is(failure?.code, "ERR_RABBITMQ_AUTH", "a password refused before tuning is an auth failure");
  is(failure?.replyCode, 403, "with the broker's code");
  ok(!connection.usable, "and the connection is not usable");
}

{
  // A broker that will not speak 0-9-1 answers the protocol header with its own.
  const pair = loopback();
  const broker = new Broker(pair.server);
  const connection = new Connection(pair.client, {});
  const opening = connection.open();
  await broker.sendRaw(PROTOCOL_HEADER);
  let failure = null;
  try {
    await opening;
  } catch (e) {
    failure = e;
  }
  is(failure?.code, "ERR_RABBITMQ_PROTOCOL", "an unusable protocol header is a protocol failure");
}

{
  // A broker that offers only mechanisms we do not have.
  const pair = loopback();
  const broker = new Broker(pair.server);
  const connection = new Connection(pair.client, {});
  const opening = connection.open();
  await broker.send("connection.start", {
    versionMajor: 0,
    versionMinor: 9,
    serverProperties: new Map(),
    mechanisms: "EXTERNAL",
    locales: "en_US",
  });
  let failure = null;
  try {
    await opening;
  } catch (e) {
    failure = e;
  }
  is(failure?.code, "ERR_RABBITMQ_HANDSHAKE", "an unsupported mechanism is a handshake failure");
}

{
  // The socket ending without a `connection.close` is a lost connection.
  const pair = loopback();
  const broker = new Broker(pair.server);
  const connection = new Connection(pair.client, {});
  const opening = connection.open();
  await broker.awaitHeader();
  broker.end();
  let failure = null;
  try {
    await opening;
  } catch (e) {
    failure = e;
  }
  is(failure?.code, "ERR_RABBITMQ_CONNECTION", "an abrupt end is a connection failure");
}

{
  // Bytes that are not a frame are fatal, and every open channel hears about it.
  const pair = loopback();
  const broker = new Broker(pair.server);
  const connection = new Connection(pair.client, { heartbeat: 0 });
  const opening = connection.open();
  await handshake(broker);
  await opening;

  const handler = recorder();
  const opened = connection.openChannel(handler);
  await broker.expect("channel.open");
  await broker.send("channel.open-ok", {}, 1);
  await opened;

  // A frame claiming a type that does not exist.
  await broker.sendRaw([0x07, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xce]);
  await new Promise((resolve) => setTimeout(resolve, 5));
  is(handler.closed.length, 1, "the channel is closed by the failure");
  is(handler.closed[0].error.code, "ERR_RABBITMQ_PROTOCOL", "with the protocol failure");
  ok(!connection.usable, "and the connection is dead");
}

// -- a deliberate close -----------------------------------------------------

{
  const pair = loopback();
  const broker = new Broker(pair.server);
  let notified = null;
  const connection = new Connection(pair.client, { heartbeat: 0 });
  connection.onClosed = (error) => {
    notified = error;
  };
  const opening = connection.open();
  await handshake(broker);
  await opening;

  const closing = connection.close();
  const close = await broker.expect("connection.close");
  is(close.args.replyCode, 200, "a deliberate close says 200");
  await broker.send("connection.close-ok");
  await closing;
  ok(!connection.usable, "the connection is closed");
  ok(notified === null, "a deliberate close is not reported as a failure");
}

// -- blocked and unblocked --------------------------------------------------

{
  const pair = loopback();
  const broker = new Broker(pair.server);
  let blocked = null;
  let unblocked = false;
  const connection = new Connection(pair.client, { heartbeat: 0 });
  connection.onBlocked = (reason) => {
    blocked = reason;
  };
  connection.onUnblocked = () => {
    unblocked = true;
  };
  const opening = connection.open();
  await handshake(broker);
  await opening;

  await broker.send("connection.blocked", { reason: "low on disk" });
  await new Promise((resolve) => setTimeout(resolve, 5));
  is(blocked, "low on disk", "connection.blocked is surfaced with its reason");
  await broker.send("connection.unblocked", {});
  await new Promise((resolve) => setTimeout(resolve, 5));
  ok(unblocked, "connection.unblocked is surfaced");
  await shut(connection, broker);
}

// -- an answer that overtakes its write -------------------------------------

// A socket under backpressure settles a write after the peer has already read
// it and answered. The waiter for that answer has to exist before the send, or
// the answer is dropped and the handshake waits out its timeout for a reply
// that came. Here every client write settles 30ms late.
{
  const pair = loopback();
  const broker = new Broker(pair.server);
  const slow = {
    read: () => pair.client.read(),
    write: async (bytes) => {
      await pair.client.write(bytes);
      await new Promise((resolve) => setTimeout(resolve, 30));
    },
    close: () => pair.client.close(),
  };
  const connection = new Connection(slow, { heartbeat: 0, handshakeTimeout: 500, timeout: 500 });
  const opening = connection.open();
  await handshake(broker);
  let failure = null;
  await opening.catch((e) => {
    failure = e;
  });
  is(failure?.message ?? null, null, "an answer read before its write settled is not lost");

  const channelOpening = connection.openChannel({
    onMethod() {},
    onHeader() {},
    onBody() {},
    onClosed() {},
  });
  const open = await broker.expect("channel.open");
  await broker.send("channel.open-ok", {}, open.channel);
  is(await channelOpening, open.channel, "nor is a channel.open-ok");
  await shut(connection, broker);
}

// -- the agreed frame size reaches the decoder ------------------------------

// The decoder starts at 128 KiB. A broker that agrees a larger frame-max may
// send frames that large, and they must be read rather than refused.
{
  const pair = loopback();
  const broker = new Broker(pair.server);
  const connection = new Connection(pair.client, { heartbeat: 0 });
  const opening = connection.open();
  await handshake(broker, { channelMax: 0, frameMax: 1 << 20, heartbeat: 0 });
  await opening;
  is(connection.frameMax, 1 << 20, "the broker's larger frame-max is agreed");
  const bodies = [];
  const channelOpening = connection.openChannel({
    onMethod() {},
    onHeader() {},
    onBody(_n, bytes) {
      bodies.push(bytes.length);
    },
    onClosed() {},
  });
  const open = await broker.expect("channel.open");
  await broker.send("channel.open-ok", {}, open.channel);
  await channelOpening;
  await broker.sendBody(new Uint8Array(300_000), open.channel);
  await new Promise((resolve) => setTimeout(resolve, 20));
  is(bodies, [300_000], "a 300 KB frame under the agreed maximum is read");
  ok(connection.usable, "and the connection survives it");
  await shut(connection, broker);
}

// -- a broker's close is answered before the socket goes ---------------------

{
  const pair = loopback();
  const broker = new Broker(pair.server);
  let closed = false;
  const transport = {
    read: () => pair.client.read(),
    write: (bytes) => pair.client.write(bytes),
    close: async () => {
      closed = true;
      await pair.client.close();
    },
  };
  const connection = new Connection(transport, { heartbeat: 0 });
  const opening = connection.open();
  await handshake(broker);
  await opening;
  const sentBefore = [];
  const realWrite = transport.write;
  transport.write = async (bytes) => {
    sentBefore.push(closed);
    return realWrite(bytes);
  };
  await broker.send("connection.close", {
    replyCode: 320,
    replyText: "CONNECTION_FORCED",
    classId: 0,
    methodId: 0,
  });
  await broker.expect("connection.close-ok");
  await new Promise((resolve) => setTimeout(resolve, 20));
  is(sentBefore, [false], "close-ok is written before the socket is closed");
  ok(closed, "and the socket is closed after it");
  is(connection.failure?.replyCode, 320, "the broker's reason is kept");
}

// -- a channel.open that times out keeps its number until it is closed -------

{
  const pair = loopback();
  const broker = new Broker(pair.server);
  const connection = new Connection(pair.client, { heartbeat: 0, timeout: 40 });
  const opening = connection.open();
  await handshake(broker);
  await opening;
  const handler = { onMethod() {}, onHeader() {}, onBody() {}, onClosed() {} };
  let failure = null;
  const first = connection.openChannel(handler).catch((e) => {
    failure = e;
  });
  const open = await broker.expect("channel.open");
  await first;
  is(failure?.code, "ERR_RABBITMQ_TIMEOUT", "an unanswered channel.open times out");
  const close = await broker.expect("channel.close");
  is(close.channel, open.channel, "and the channel is closed, in case the broker opened it late");
  ok(connection.hasChannel(open.channel), "its number stays taken until the broker agrees");
  const second = connection.openChannel(handler);
  const open2 = await broker.expect("channel.open");
  ok(open2.channel !== open.channel, "so the next channel gets another number");
  await broker.send("channel.open-ok", {}, open2.channel);
  await second;
  await broker.send("channel.close-ok", {}, open.channel);
  await new Promise((resolve) => setTimeout(resolve, 10));
  ok(!connection.hasChannel(open.channel), "close-ok frees the number");
  await shut(connection, broker);
}

if (report("connection") > 0) exit(1);
