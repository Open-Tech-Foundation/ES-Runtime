// The connection: URL parsing, the opening handshake, the 5.0→3.1.1 fallback,
// the protocol's own acknowledgements, and the failure latch — all against a
// scripted broker, with no socket anywhere.
import { exit } from "runtime:process";
import {
  Connection,
  DEFAULT_KEEP_ALIVE,
  DEFAULT_MQTT_PORT,
  DEFAULT_MQTTS_PORT,
  openWithDial,
  parseMqttUrl,
} from "../../dist/connection.js";
import { MqttErrorCode } from "../../dist/errors.js";
import { PacketType } from "../../dist/protocol/packets.js";
import { ok, report, throws } from "./assert.mjs";
import { fakeBroker } from "./broker.mjs";

const ENCODER = new TextEncoder();
const DECODER = new TextDecoder();

function tick(ms = 10) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

// --- URL parsing -----------------------------------------------------------

const plain = parseMqttUrl("mqtt://broker.example");
ok(
  plain.hostname === "broker.example" && plain.port === DEFAULT_MQTT_PORT,
  "mqtt:// defaults to 1883",
);
ok(plain.tls === false, "mqtt:// is not TLS");
const secure = parseMqttUrl("mqtts://broker.example");
ok(secure.port === DEFAULT_MQTTS_PORT && secure.tls === true, "mqtts:// defaults to 8883 and TLS");
ok(parseMqttUrl("mqtt://host:1884").port === 1884, "an explicit port wins");
ok(parseMqttUrl("mqtts://host:1234").port === 1234, "an explicit port wins over the TLS default");
ok(parseMqttUrl("tcp://host").tls === false, "tcp:// is plain");
ok(parseMqttUrl("ssl://host").tls === true, "ssl:// is TLS");

const creds = parseMqttUrl("mqtt://alice:secret@host:1883");
ok(creds.username === "alice" && creds.password === "secret", "user and password parse");
ok(parseMqttUrl("mqtt://alice@host").username === "alice", "a user with no password parses");
ok(parseMqttUrl("mqtt://alice@host").password === undefined, "and has no password");
const encoded = parseMqttUrl("mqtt://a%40b:p%3Ass@host");
ok(encoded.username === "a@b" && encoded.password === "p:ss", "percent-encoding is decoded");
const ipv6 = parseMqttUrl("mqtt://[::1]:1883");
ok(ipv6.hostname === "::1" && ipv6.port === 1883, "a bracketed IPv6 host parses");
ok(parseMqttUrl("mqtt://[::1]").port === 1883, "a bracketed IPv6 host takes the default port");

await throws(() => parseMqttUrl("http://host"), "an unknown scheme is refused");
await throws(() => parseMqttUrl("mqtt://"), "a URL with no host is refused");
await throws(() => parseMqttUrl("mqtt://host:notaport"), "a non-numeric port is refused");
await throws(() => parseMqttUrl("mqtt://host:99999"), "an out-of-range port is refused");
await throws(() => parseMqttUrl("nonsense"), "a string that is not a URL is refused");

ok(DEFAULT_KEEP_ALIVE === 60, "the default keep-alive is 60s");

// --- the handshake ---------------------------------------------------------

{
  const broker = fakeBroker(5);
  const pending = Connection.over(
    broker.transport,
    {
      clientId: "client-1",
      username: "u",
      password: "p",
      keepAlive: 30,
      cleanStart: true,
      will: { topic: "will/topic", payload: Uint8Array.of(1), qos: 1, retain: true },
      sessionExpiryInterval: 120,
    },
    {},
    5,
  );
  const connect = await broker.next();
  ok(connect.type === PacketType.Connect, "the client sends a CONNECT first");
  ok(connect.clientId === "client-1", "the client identifier is sent");
  ok(connect.username === "u", "the username is sent");
  ok(DECODER.decode(connect.password) === "p", "the password is sent as bytes");
  ok(connect.keepAlive === 30, "the keep-alive is sent");
  ok(connect.cleanStart === true, "clean start is sent");
  ok(connect.will.topic === "will/topic" && connect.will.qos === 1, "the will is sent");
  ok(connect.properties.sessionExpiryInterval === 120, "the session expiry is sent");

  broker.send({
    type: PacketType.Connack,
    sessionPresent: true,
    reasonCode: 0,
    properties: { assignedClientIdentifier: "assigned", serverKeepAlive: 45 },
  });
  const conn = await pending;
  ok(conn.protocolLevel === 5, "the connection is MQTT 5.0");
  ok(conn.sessionPresent === true, "the broker's session-present flag is reported");
  ok(conn.keepAlive === 45, "the broker's server keep-alive overrides the request");
  ok(conn.connack.properties.assignedClientIdentifier === "assigned", "the CONNACK is available");

  const closing = conn.close();
  const disconnect = await broker.next();
  ok(disconnect.type === PacketType.Disconnect, "close sends a DISCONNECT");
  await closing;
  ok(conn.closed, "a deliberate close marks the connection closed");
  ok((await broker.next()) === null, "and then the pipe ends");
}

// --- a refusing broker -----------------------------------------------------

{
  const broker = fakeBroker(5);
  const pending = Connection.over(broker.transport, { clientId: "c" }, {}, 5);
  await broker.next();
  broker.send({ type: PacketType.Connack, sessionPresent: false, reasonCode: 0x87 });
  let error = null;
  try {
    await pending;
  } catch (e) {
    error = e;
  }
  ok(error !== null && error.code === MqttErrorCode.Auth, "a refusal is an auth error");
  ok(
    error.reasonCode === 0x87 && error.reasonName === "Not authorized",
    "carrying the broker's code and name",
  );
}

// --- a silent broker times the handshake out -------------------------------

{
  const broker = fakeBroker(5);
  const pending = Connection.over(broker.transport, { clientId: "c", timeout: 60 }, {}, 5);
  await broker.next();
  // The broker accepted the socket and then says nothing at all. The read must
  // be raced against the deadline, or this never returns.
  let error = null;
  try {
    await pending;
  } catch (e) {
    error = e;
  }
  ok(
    error !== null && error.code === MqttErrorCode.Timeout,
    "a broker that never answers times the handshake out",
  );
}

// --- a 3.1.1-style answer to a 5.0 offer -----------------------------------

{
  const broker = fakeBroker(5);
  const pending = Connection.over(broker.transport, { clientId: "c" }, {}, 5);
  await broker.next();
  // A 3.1.1 broker replies with return code 1, which has no 5.0 CONNACK meaning.
  broker.send({ type: PacketType.Connack, sessionPresent: false, reasonCode: 1 });
  let error = null;
  try {
    await pending;
  } catch (e) {
    error = e;
  }
  ok(
    error !== null && error.code === MqttErrorCode.Handshake,
    "a legacy success code is a version mismatch",
  );
}

// --- the fallback to 3.1.1 -------------------------------------------------

{
  const brokers = [];
  const dial = async () => {
    const broker = fakeBroker(brokers.length === 0 ? 5 : 4);
    brokers.push(broker);
    return broker.transport;
  };
  const pending = openWithDial("mqtt://broker:1883", { clientId: "c", keepAlive: 0 }, {}, dial);
  const first = await brokers[0].next();
  ok(first.type === PacketType.Connect, "the first attempt is a CONNECT");
  brokers[0].send({ type: PacketType.Connack, sessionPresent: false, reasonCode: 0x84 });
  await tick();
  ok(brokers.length === 2, "the client dials a second time after an unsupported-version refusal");
  const second = await brokers[1].next();
  ok(second.type === PacketType.Connect, "the second attempt is also a CONNECT");
  brokers[1].send({ type: PacketType.Connack, sessionPresent: false, reasonCode: 0 });
  const conn = await pending;
  ok(conn.protocolLevel === 4, "and it settles on MQTT 3.1.1");
  await conn.close();
}

// --- a 3.1.1 handshake on its own ------------------------------------------

{
  const broker = fakeBroker(4);
  const pending = Connection.over(broker.transport, { clientId: "c", keepAlive: 0 }, {}, 4);
  await broker.next();
  broker.send({ type: PacketType.Connack, sessionPresent: false, reasonCode: 0 });
  const conn = await pending;
  ok(conn.protocolLevel === 4, "a forced 3.1.1 connection is 3.1.1");
  ok(conn.connack.properties === undefined, "a 3.1.1 CONNACK carries no properties");
  await conn.close();
}

// --- packet identifiers ----------------------------------------------------

{
  const broker = fakeBroker(5);
  const pending = Connection.over(broker.transport, { clientId: "c", keepAlive: 0 }, {}, 5);
  await broker.next();
  broker.send({ type: PacketType.Connack, sessionPresent: false, reasonCode: 0 });
  const conn = await pending;
  ok(conn.allocatePacketId() === 1, "the first packet identifier is 1");
  ok(conn.allocatePacketId() === 2, "then 2");
  await conn.close();
}

// --- the protocol's own acknowledgements -----------------------------------

{
  const messages = [];
  const accepts = [];
  const acks = [];
  const broker = fakeBroker(5);
  // Counts writes, so "nothing was acknowledged yet" is checked without a
  // second reader racing the broker's.
  let writes = 0;
  const transport = {
    read: () => broker.transport.read(),
    write: (bytes) => {
      writes++;
      return broker.transport.write(bytes);
    },
    close: () => broker.transport.close(),
  };
  const pending = Connection.over(
    transport,
    { clientId: "c", keepAlive: 0 },
    {
      onMessage: (packet, accept) => {
        messages.push(packet);
        accepts.push(accept);
      },
      onAck: (packet) => acks.push(packet),
    },
    5,
  );
  const connect = await broker.next();
  ok(
    connect.properties?.receiveMaximum === 250,
    "the client advertises its Receive Maximum (250 by default)",
  );
  broker.send({ type: PacketType.Connack, sessionPresent: false, reasonCode: 0 });
  const conn = await pending;

  // Inbound QoS 1: delivered at once, acknowledged only when accepted — PUBACK
  // is sent "having accepted ownership" (4.3.2).
  const quiet = writes;
  broker.send({
    type: PacketType.Publish,
    topic: "a/b",
    payload: ENCODER.encode("hello"),
    qos: 1,
    retain: false,
    dup: false,
    packetId: 42,
  });
  await tick();
  ok(
    messages.length === 1 && DECODER.decode(messages[0].payload) === "hello",
    "a QoS 1 message is delivered",
  );
  ok(writes === quiet, "but not acknowledged before it is accepted");
  accepts[0]();
  accepts[0]();
  const puback = await broker.next();
  ok(puback.type === PacketType.Puback && puback.packetId === 42, "accepting it sends the PUBACK");
  await tick();
  ok(writes === quiet + 1, "once, however often accept is called");

  // Inbound QoS 2: delivered on its PUBLISH, a DUP before acceptance is the same
  // message, PUBREC on acceptance, PUBCOMP on PUBREL.
  const qos2 = {
    type: PacketType.Publish,
    topic: "a/b",
    payload: ENCODER.encode("once"),
    qos: 2,
    retain: false,
    dup: false,
    packetId: 7,
  };
  broker.send(qos2);
  broker.send({ ...qos2, dup: true });
  await tick();
  ok(
    messages.length === 2 && DECODER.decode(messages[1].payload) === "once",
    "a QoS 2 message is delivered once",
  );
  accepts[1]();
  const pubrec = await broker.next();
  ok(pubrec.type === PacketType.Pubrec && pubrec.packetId === 7, "accepting it sends the PUBREC");
  // Retransmitted after acceptance: only the PUBREC is owed again.
  broker.send({ ...qos2, dup: true });
  const again = await broker.next();
  ok(
    again.type === PacketType.Pubrec && again.packetId === 7,
    "a later DUP draws the PUBREC again",
  );
  await tick();
  ok(messages.length === 2, "and is not delivered again");
  broker.send({ type: PacketType.Pubrel, packetId: 7, reasonCode: 0 });
  const pubcomp = await broker.next();
  ok(pubcomp.type === PacketType.Pubcomp && pubcomp.packetId === 7, "PUBREL draws a PUBCOMP");

  // Outbound QoS 2 step: a PUBREC from the broker draws a PUBREL, and the PUBCOMP is reported.
  broker.send({ type: PacketType.Pubrec, packetId: 9, reasonCode: 0 });
  const pubrel = await broker.next();
  ok(pubrel.type === PacketType.Pubrel && pubrel.packetId === 9, "a PUBREC draws a PUBREL");
  broker.send({ type: PacketType.Pubcomp, packetId: 9, reasonCode: 0 });
  await tick();
  ok(
    acks.some((packet) => packet.type === PacketType.Pubcomp && packet.packetId === 9),
    "the PUBCOMP is reported to the client",
  );

  // A negative PUBREC ends the exchange: it is reported, and no PUBREL is sent,
  // so the packet identifier is free for reuse at once.
  broker.send({ type: PacketType.Pubrec, packetId: 11, reasonCode: 0x80 });
  await tick();
  ok(
    acks.some((packet) => packet.type === PacketType.Pubrec && packet.packetId === 11),
    "a negative PUBREC is reported to the client",
  );
  await conn.send({ type: PacketType.Pingreq });
  const after = await broker.next();
  ok(after.type === PacketType.Pingreq, "and no PUBREL is sent for a negative PUBREC");

  await conn.close();
}

// --- ping ------------------------------------------------------------------

{
  const broker = fakeBroker(5);
  const pending = Connection.over(broker.transport, { clientId: "c", keepAlive: 0 }, {}, 5);
  await broker.next();
  broker.send({ type: PacketType.Connack, sessionPresent: false, reasonCode: 0 });
  const conn = await pending;

  const pong = conn.ping(2000);
  const pingreq = await broker.next();
  ok(pingreq.type === PacketType.Pingreq, "ping sends a PINGREQ");
  // Unrelated inbound traffic must not stand in for the PINGRESP.
  broker.send({
    type: PacketType.Publish,
    topic: "a/b",
    payload: ENCODER.encode("x"),
    qos: 0,
    retain: false,
    dup: false,
  });
  let resolved = false;
  void pong.then(() => {
    resolved = true;
  });
  await tick(60);
  ok(!resolved, "unrelated traffic does not resolve ping");
  broker.send({ type: PacketType.Pingresp });
  await pong;
  ok(resolved, "the PINGRESP resolves ping");

  await conn.close();
}

// --- concurrent pings share one exchange -----------------------------------

{
  const broker = fakeBroker(5);
  const pending = Connection.over(broker.transport, { clientId: "c", keepAlive: 0 }, {}, 5);
  await broker.next();
  broker.send({ type: PacketType.Connack, sessionPresent: false, reasonCode: 0 });
  const conn = await pending;

  const first = conn.ping(2000);
  const second = conn.ping(2000);
  const pingreq = await broker.next();
  ok(pingreq.type === PacketType.Pingreq, "a first ping sends a PINGREQ");
  // A PINGRESP has no identifier, so a second PINGREQ could not be told apart;
  // the second call must join the one in flight rather than send its own.
  const raced = await Promise.race([
    broker.next().then(() => "packet"),
    tick(80).then(() => "none"),
  ]);
  ok(raced === "none", "a second, concurrent ping sends no second PINGREQ");
  broker.send({ type: PacketType.Pingresp });
  await Promise.all([first, second]);
  ok(true, "one PINGRESP resolves both callers");

  await conn.close();
}

// --- a PINGRESP that never comes is a lost connection ----------------------

{
  const failures = [];
  const broker = fakeBroker(5);
  const pending = Connection.over(
    broker.transport,
    { clientId: "c", keepAlive: 0 },
    { onClosed: (error) => failures.push(error) },
    5,
  );
  await broker.next();
  broker.send({ type: PacketType.Connack, sessionPresent: false, reasonCode: 0 });
  const conn = await pending;

  let error = null;
  const pong = conn.ping(60).catch((e) => {
    error = e;
  });
  await broker.next(); // the PINGREQ, which the broker never answers
  await pong;
  ok(
    error !== null && error.code === MqttErrorCode.Connection,
    "an unanswered PINGREQ fails the ping",
  );
  ok(conn.closed && failures.length === 1, "and the connection is treated as lost, once");

  let refused = null;
  try {
    await conn.ping(2000);
  } catch (e) {
    refused = e;
  }
  ok(refused !== null, "a later ping is refused, not answered by a stale PINGRESP");
}

// --- a broker's DISCONNECT is a failure ------------------------------------

{
  const failures = [];
  const broker = fakeBroker(5);
  const pending = Connection.over(
    broker.transport,
    { clientId: "c", keepAlive: 0 },
    { onClosed: (error) => failures.push(error) },
    5,
  );
  await broker.next();
  broker.send({ type: PacketType.Connack, sessionPresent: false, reasonCode: 0 });
  const conn = await pending;
  broker.send({
    type: PacketType.Disconnect,
    reasonCode: 0x8e,
    properties: { reasonString: "bye" },
  });
  await tick();
  ok(conn.closed, "a broker DISCONNECT closes the connection");
  ok(
    failures.length === 1 && failures[0].code === MqttErrorCode.Server,
    "and is reported as a server error",
  );
  ok(failures[0].reasonCode === 0x8e, "with the broker's reason code");
  ok(conn.failure !== null, "and the failure is latched");
}

// --- a lost socket is a failure, latched once ------------------------------

{
  const failures = [];
  const broker = fakeBroker(5);
  const pending = Connection.over(
    broker.transport,
    { clientId: "c", keepAlive: 0 },
    { onClosed: (error) => failures.push(error) },
    5,
  );
  await broker.next();
  broker.send({ type: PacketType.Connack, sessionPresent: false, reasonCode: 0 });
  const conn = await pending;
  broker.close();
  await tick();
  ok(conn.closed && failures.length === 1, "a vanished broker is a failure");
  ok(failures[0].code === MqttErrorCode.Connection, "reported as a connection error");
}

// --- a 3.1.1 refusal is a refusal --------------------------------------------

// A 3.1.1 CONNACK refuses with return codes 1–5, not with the 5.0 codes from
// 0x80. Read by the 5.0 rule, a broker rejecting the password looked like one
// accepting the connection.
for (const [returnCode, expected] of [
  [1, MqttErrorCode.Handshake],
  [2, MqttErrorCode.Refused],
  [3, MqttErrorCode.Refused],
  [4, MqttErrorCode.Auth],
  [5, MqttErrorCode.Auth],
]) {
  const broker = fakeBroker(4);
  const pending = Connection.over(broker.transport, { clientId: "c", keepAlive: 0 }, {}, 4);
  await broker.next();
  broker.send({ type: PacketType.Connack, sessionPresent: false, reasonCode: returnCode });
  let failure = null;
  await pending.catch((e) => {
    failure = e;
  });
  ok(failure?.code === expected, `a 3.1.1 return code ${returnCode} rejects as ${expected}`);
  ok(failure?.reasonCode === returnCode, "carrying the return code");
}

// --- integer fields are range-checked, not wrapped -----------------------------

{
  const broker = fakeBroker(5);
  let failure = null;
  await Connection.over(broker.transport, { clientId: "c", keepAlive: 70000 }, {}, 5).catch((e) => {
    failure = e;
  });
  ok(failure?.code === MqttErrorCode.Protocol, "a keep-alive past 65535 is refused, not wrapped");
}

// --- a large packet in many chunks ------------------------------------------

{
  const broker = fakeBroker(5);
  const got = [];
  const pending = Connection.over(
    broker.transport,
    { clientId: "c", keepAlive: 0 },
    { onMessage: (packet) => got.push(packet) },
    5,
  );
  await broker.next();
  broker.send({ type: PacketType.Connack, sessionPresent: false, reasonCode: 0 });
  const conn = await pending;
  // 2 MB of payload, delivered 1 KB at a time, then a second packet in the
  // same chunk as the first one's tail.
  const payload = new Uint8Array(2 * 1024 * 1024).map((_, i) => i & 0xff);
  const { encodePacket } = await import("../../dist/protocol/packets.js");
  const big = encodePacket(
    { type: PacketType.Publish, topic: "big", payload, qos: 0, retain: false, dup: false },
    5,
  );
  const small = encodePacket(
    {
      type: PacketType.Publish,
      topic: "small",
      payload: ENCODER.encode("x"),
      qos: 0,
      retain: false,
      dup: false,
    },
    5,
  );
  const started = Date.now();
  for (let at = 0; at < big.length; at += 1024) {
    const end = Math.min(big.length, at + 1024);
    if (end === big.length) {
      const tail = new Uint8Array(end - at + small.length);
      tail.set(big.subarray(at, end));
      tail.set(small, end - at);
      broker.sendRaw(tail);
    } else {
      broker.sendRaw(big.slice(at, end));
    }
  }
  for (let i = 0; i < 200 && got.length < 2; i++) await tick(10);
  ok(
    got.length === 2,
    "a packet split into 2 048 chunks, and one sharing its last chunk, both arrive",
  );
  ok(got[0]?.payload.length === payload.length, "the large payload is whole");
  ok(got[0]?.payload[123456] === (123456 & 0xff), "and intact");
  ok(got[1]?.topic === "small", "and the packet after it is read too");
  ok(Date.now() - started < 3000, "in linear time");
  await conn.close();
}

// --- inbound topic aliases ------------------------------------------------------

{
  const broker = fakeBroker(5);
  const got = [];
  const failures = [];
  const pending = Connection.over(
    broker.transport,
    { clientId: "c", keepAlive: 0, properties: { topicAliasMaximum: 4 } },
    { onMessage: (packet) => got.push(packet.topic), onClosed: (e) => failures.push(e) },
    5,
  );
  const connect = await broker.next();
  ok(connect.properties?.topicAliasMaximum === 4, "the client advertises its alias maximum");
  broker.send({ type: PacketType.Connack, sessionPresent: false, reasonCode: 0 });
  await pending;
  const publish = (topic, alias) =>
    broker.send({
      type: PacketType.Publish,
      topic,
      payload: ENCODER.encode("x"),
      qos: 0,
      retain: false,
      dup: false,
      properties: { topicAlias: alias },
    });
  publish("sensors/a", 2);
  publish("", 2);
  await tick(20);
  ok(
    got.join(",") === "sensors/a,sensors/a",
    "an alias is set by one PUBLISH and used by the next",
  );
  publish("", 3);
  await tick(20);
  ok(
    failures[0]?.code === MqttErrorCode.Protocol,
    "an alias used before it was set is a protocol error",
  );
}

{
  // Without an advertised maximum the broker may not use aliases at all.
  const broker = fakeBroker(5);
  const failures = [];
  const pending = Connection.over(
    broker.transport,
    { clientId: "c", keepAlive: 0 },
    { onClosed: (e) => failures.push(e) },
    5,
  );
  await broker.next();
  broker.send({ type: PacketType.Connack, sessionPresent: false, reasonCode: 0 });
  await pending;
  broker.send({
    type: PacketType.Publish,
    topic: "t",
    payload: ENCODER.encode("x"),
    qos: 0,
    retain: false,
    dup: false,
    properties: { topicAlias: 1 },
  });
  await tick(20);
  ok(failures[0]?.code === MqttErrorCode.Protocol, "an alias the client never allowed is refused");
}

exit(report("connection") > 0 ? 1 : 0);
