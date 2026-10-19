// The client: publish at each QoS, subscribe and route by filter, request/reply
// over the 5.0 convention, and unsubscribe — all against the scripted broker.
import { exit } from "runtime:process";
import { Message, MqttClient } from "../../dist/client.js";
import { MqttErrorCode } from "../../dist/errors.js";
import { PacketType } from "../../dist/protocol/packets.js";
import { ok, report, throws } from "./assert.mjs";
import { fakeBroker } from "./broker.mjs";

const ENCODER = new TextEncoder();

const tick = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function connected(level = 5, options = {}) {
  const broker = fakeBroker(level);
  // Writes can be stalled, so a test can hold one past an operation's timeout.
  let stalled = false;
  const transport = {
    read: () => broker.transport.read(),
    write: (bytes) => (stalled ? new Promise(() => {}) : broker.transport.write(bytes)),
    close: () => broker.transport.close(),
  };
  const pending = MqttClient.over(transport, { clientId: "c", keepAlive: 0, ...options }, level);
  const connect = await broker.next();
  ok(connect.type === PacketType.Connect, "the client sends a CONNECT");
  broker.send({ type: PacketType.Connack, sessionPresent: false, reasonCode: 0 });
  const client = await pending;
  return {
    broker,
    client,
    stall() {
      stalled = true;
    },
  };
}

function publishTo(broker, topic, payload, extra = {}) {
  broker.send({
    type: PacketType.Publish,
    topic,
    payload: ENCODER.encode(payload),
    qos: 0,
    retain: false,
    dup: false,
    ...extra,
  });
}

async function subscribe(client, broker, filter, options = {}, reasons) {
  const pending = client.subscribe(filter, options);
  const packet = await broker.next();
  broker.send({
    type: PacketType.Suback,
    packetId: packet.packetId,
    reasons: reasons ?? [options.qos ?? 0],
  });
  return { subscription: await pending, request: packet };
}

// --- publish QoS 0 ---------------------------------------------------------

{
  const { broker, client } = await connected();
  ok(client.protocolLevel === 5, "a default connection is MQTT 5.0");
  await client.publish("a/b", "hello");
  const packet = await broker.next();
  ok(packet.type === PacketType.Publish, "publish sends a PUBLISH");
  ok(packet.topic === "a/b" && packet.qos === 0, "with the topic and QoS 0");
  ok(new TextDecoder().decode(packet.payload) === "hello", "and the payload");
  ok(packet.packetId === undefined, "QoS 0 carries no packet identifier");

  await client.publish("a/b", "retained", { retain: true });
  const retained = await broker.next();
  ok(retained.retain === true, "retain is sent");

  await client.publish("a/b", "typed", {
    contentType: "text/plain",
    userProperties: [["k", "v"]],
  });
  const typed = await broker.next();
  ok(typed.properties.contentType === "text/plain", "content type is sent as a property (5.0)");
  ok(typed.properties.userProperties[0][0] === "k", "user properties are sent");

  await throws(() => client.publish("a/+", "x"), "publishing to a wildcard topic is refused");
  await client.close();
}

// --- publish QoS 1 ---------------------------------------------------------

{
  const { broker, client } = await connected();
  const pending = client.publish("a/b", "one", { qos: 1 });
  const packet = await broker.next();
  ok(
    packet.qos === 1 && typeof packet.packetId === "number",
    "a QoS 1 publish has a packet identifier",
  );
  broker.send({ type: PacketType.Puback, packetId: packet.packetId, reasonCode: 0 });
  await pending;
  ok(true, "the publish resolves on PUBACK");
  await client.close();
}

{
  const { broker, client } = await connected();
  const pending = client.publish("a/b", "one", { qos: 1 });
  const packet = await broker.next();
  broker.send({ type: PacketType.Puback, packetId: packet.packetId, reasonCode: 0x87 });
  let error = null;
  try {
    await pending;
  } catch (e) {
    error = e;
  }
  ok(error !== null && error.code === MqttErrorCode.Publish, "a refusal rejects the publish");
  ok(error.reasonCode === 0x87, "with the broker's reason code");
  await client.close();
}

// --- publish QoS 2 ---------------------------------------------------------

{
  const { broker, client } = await connected();
  const pending = client.publish("a/b", "two", { qos: 2 });
  const packet = await broker.next();
  ok(packet.qos === 2, "a QoS 2 publish is sent at QoS 2");
  broker.send({ type: PacketType.Pubrec, packetId: packet.packetId, reasonCode: 0 });
  const pubrel = await broker.next();
  ok(
    pubrel.type === PacketType.Pubrel && pubrel.packetId === packet.packetId,
    "PUBREC draws a PUBREL",
  );
  broker.send({ type: PacketType.Pubcomp, packetId: packet.packetId, reasonCode: 0 });
  await pending;
  ok(true, "the publish resolves on PUBCOMP");
  await client.close();
}

{
  const { broker, client } = await connected();
  const pending = client.publish("a/b", "two", { qos: 2 });
  const packet = await broker.next();
  broker.send({ type: PacketType.Pubrec, packetId: packet.packetId, reasonCode: 0x80 });
  let error = null;
  try {
    await pending;
  } catch (e) {
    error = e;
  }
  ok(
    error !== null && error.code === MqttErrorCode.Publish,
    "a negative PUBREC fails the QoS 2 publish",
  );
  ok(error.reasonCode === 0x80, "with the broker's reason code");

  // No PUBREL follows a negative PUBREC, so the next packet is the next thing
  // the client sends — proving the exchange ended rather than carrying on.
  const again = client.publish("a/b", "one", { qos: 1 });
  const next = await broker.next();
  ok(next.type === PacketType.Publish && next.qos === 1, "and no PUBREL is sent for it");
  broker.send({ type: PacketType.Puback, packetId: next.packetId, reasonCode: 0 });
  await again;
  await client.close();
}

// --- subscribe, and routing by filter --------------------------------------

{
  const { broker, client } = await connected();
  const { subscription } = await subscribe(client, broker, "sensor/+", { qos: 1 });
  ok(subscription.grantedQos === 1, "the granted QoS is reported");

  publishTo(broker, "sensor/temp", "21");
  const message = await subscription.nextMessage(1000);
  ok(message.topic === "sensor/temp" && message.text() === "21", "a matching message is delivered");

  publishTo(broker, "other/temp", "no");
  let timedOut = false;
  try {
    await subscription.nextMessage(80);
  } catch (e) {
    timedOut = e.code === MqttErrorCode.Timeout;
  }
  ok(timedOut, "a non-matching topic is not delivered");

  // A timed-out nextMessage must leave nothing behind: the waiter it queued has
  // to be withdrawn, or the *next* message resolves that abandoned waiter and is
  // lost instead of delivered here.
  publishTo(broker, "sensor/temp", "22");
  const after = await subscription.nextMessage(1000);
  ok(after.text() === "22", "a message after a timeout is still delivered, not lost");

  await client.close();
}

{
  const { broker, client } = await connected();
  const pending = client.subscribe("slow", { timeout: 60 });
  await broker.next();
  // No SUBACK: the subscribe must fail on its timeout, and the client must stay
  // usable — the abandoned subscription is not left registered.
  let error = null;
  try {
    await pending;
  } catch (e) {
    error = e;
  }
  ok(
    error !== null && error.code === MqttErrorCode.Timeout,
    "a missing SUBACK times the subscribe out",
  );

  const { subscription } = await subscribe(client, broker, "slow", { qos: 0 });
  publishTo(broker, "slow", "late");
  const message = await subscription.nextMessage(1000);
  ok(message.text() === "late", "and the client still subscribes afterwards");
  await client.close();
}

{
  const { broker, client } = await connected();
  const pending = client.subscribe("nope", {});
  const packet = await broker.next();
  broker.send({ type: PacketType.Suback, packetId: packet.packetId, reasons: [0x8f] });
  let error = null;
  try {
    await pending;
  } catch (e) {
    error = e;
  }
  ok(error !== null && error.code === MqttErrorCode.Subscription, "a refused subscription throws");
  await client.close();
}

// --- async iteration -------------------------------------------------------

{
  const { broker, client } = await connected();
  const { subscription } = await subscribe(client, broker, "events");
  publishTo(broker, "events", "one");
  publishTo(broker, "events", "two");
  const seen = [];
  for await (const message of subscription) {
    seen.push(message.text());
    if (seen.length === 2) break;
  }
  ok(seen.join() === "one,two", "the subscription is async-iterable");
  await client.close();
}

// --- unsubscribe -----------------------------------------------------------

{
  const { broker, client } = await connected();
  const { subscription } = await subscribe(client, broker, "a/b");
  const pending = client.unsubscribe("a/b");
  const packet = await broker.next();
  ok(
    packet.type === PacketType.Unsubscribe && packet.filters[0] === "a/b",
    "unsubscribe sends an UNSUBSCRIBE",
  );
  broker.send({ type: PacketType.Unsuback, packetId: packet.packetId, reasons: [0] });
  await pending;
  ok(subscription.closed, "the subscription ends");
  await client.close();
}

// --- a stuck write does not outlast the timeout ----------------------------

// The acknowledgement waiter is armed before the packet is written. A write that
// stalls must not keep the call waiting on a waiter that has already given up:
// each of the three paths has to settle, with the timeout error, on its own.
for (const [name, call] of [
  ["publish", (client) => client.publish("a/b", "x", { qos: 1, timeout: 60 })],
  ["subscribe", (client) => client.subscribe("a/#", { timeout: 60 })],
  ["unsubscribe", (client) => client.unsubscribe("a/#", 60)],
]) {
  const { client, stall } = await connected();
  stall();
  let error = null;
  const settled = call(client).catch((e) => {
    error = e;
  });
  const outcome = await Promise.race([settled.then(() => "settled"), tick(400).then(() => "hung")]);
  ok(
    outcome === "settled" && error !== null && error.code === MqttErrorCode.Timeout,
    `${name} rejects at its timeout even when the write stalls`,
  );
}

// --- a timed-out write is failed, not left blocked or sent -----------------

// When the timeout wins a write that is still in flight, the connection has to
// be failed: the write, stuck in the serialised chain, must not block the close,
// and the packet must not reach the broker once the write resumes.
{
  const broker = fakeBroker(5);
  let held = null;
  let stalled = false;
  let writes = 0;
  let closedWith = null;
  const transport = {
    read: () => broker.transport.read(),
    write: (bytes) => {
      writes++;
      if (!stalled) return broker.transport.write(bytes);
      // Hold this write; the test releases it later, standing in for a socket
      // that resumes after the client has already given up.
      return new Promise((resolve) => {
        held = () => {
          broker.transport.write(bytes);
          resolve();
        };
      });
    },
    close: (options) => {
      closedWith = options ?? null;
      return broker.transport.close();
    },
  };
  const pending = MqttClient.over(transport, { clientId: "c", keepAlive: 0 }, 5);
  await broker.next();
  broker.send({ type: PacketType.Connack, sessionPresent: false, reasonCode: 0 });
  const client = await pending;

  stalled = true;
  let error = null;
  await client.publish("a/b", "x", { qos: 1, timeout: 60 }).catch((e) => {
    error = e;
  });
  ok(error !== null && error.code === MqttErrorCode.Timeout, "a stalled publish still times out");
  const written = writes;
  ok(written === 2, "only the CONNECT and the stalled publish were written");
  // A write left in flight is aborted, not gracefully closed: on a real socket a
  // graceful close would wait on the stuck write and never tear the socket down.
  ok(closedWith?.abort === true, "a timed-out write aborts the transport");

  // The stuck write must not hold the serialised queue: closing must not wait on
  // a write that may never finish.
  const outcome = await Promise.race([
    client.close().then(() => "closed"),
    tick(200).then(() => "hung"),
  ]);
  ok(outcome === "closed", "close is not blocked by the stalled write");

  // The connection was failed, so releasing the write sends nothing further and
  // the packet never reaches the broker.
  ok(held !== null, "the stalled write was held");
  held();
  await tick(50);
  ok(writes === written, "no packet is written after the connection was failed");
  const late = await Promise.race([
    broker.next().then((packet) => (packet === null ? "ended" : "packet")),
    tick(50).then(() => "none"),
  ]);
  ok(late !== "packet", "the broker receives nothing after the connection was failed");
}

// --- an explicit close during a pending write ------------------------------

// close() is deliberate, not a failure. It rejects the in-flight operation with
// Closed, but must not fail the connection: a failed connection skips the
// DISCONNECT and reports an error the caller never asked for.
{
  const broker = fakeBroker(5);
  let held = null;
  let holdNext = false;
  let aborted = false;
  const transport = {
    read: () => broker.transport.read(),
    write: (bytes) => {
      // Hold exactly the next write — the publish — so the close's DISCONNECT,
      // queued behind it, still goes out once the write drains.
      if (!holdNext) return broker.transport.write(bytes);
      holdNext = false;
      return new Promise((resolve) => {
        held = () => {
          broker.transport.write(bytes);
          resolve();
        };
      });
    },
    close: (options) => {
      if (options?.abort === true) aborted = true;
      return broker.transport.close();
    },
  };
  const pending = MqttClient.over(transport, { clientId: "c", keepAlive: 0 }, 5);
  await broker.next();
  broker.send({ type: PacketType.Connack, sessionPresent: false, reasonCode: 0 });
  const client = await pending;

  holdNext = true;
  let error = null;
  const publishing = client.publish("a/b", "x", { qos: 1, timeout: 5000 }).catch((e) => {
    error = e;
  });
  await tick(0);
  ok(held !== null, "the publish write is held");

  const closing = client.close();
  await publishing;
  ok(
    error !== null && error.code === MqttErrorCode.Closed,
    "a close during a pending write rejects the publish as Closed",
  );
  ok(client.failure === null, "and is not turned into a connection failure");

  // The pending write drains, then the close takes its documented path: a
  // DISCONNECT and the socket, not an abort.
  held();
  await closing;
  ok(aborted === false, "the transport is not aborted");
  const packets = [];
  for (;;) {
    const packet = await broker.next();
    if (packet === null) break;
    packets.push(packet);
  }
  ok(
    packets.some((packet) => packet.type === PacketType.Disconnect),
    "and a DISCONNECT is sent",
  );
}

// --- request / reply -------------------------------------------------------

{
  const { broker, client } = await connected();
  const pending = client.request("service/ping", "ping?");

  const sub = await broker.next();
  ok(sub.type === PacketType.Subscribe, "request subscribes to a response topic");
  const responseTopic = sub.subscriptions[0].filter;
  broker.send({ type: PacketType.Suback, packetId: sub.packetId, reasons: [0] });

  const request = await broker.next();
  ok(
    request.type === PacketType.Publish && request.topic === "service/ping",
    "then publishes the request",
  );
  ok(request.properties.responseTopic === responseTopic, "carrying its response topic");
  ok(request.properties.correlationData.length === 8, "and correlation data");

  broker.send({
    type: PacketType.Publish,
    topic: responseTopic,
    payload: ENCODER.encode("pong"),
    qos: 0,
    retain: false,
    dup: false,
    properties: { correlationData: request.properties.correlationData },
  });

  const unsub = await broker.next();
  ok(unsub.type === PacketType.Unsubscribe, "the response subscription is dropped afterwards");
  broker.send({ type: PacketType.Unsuback, packetId: unsub.packetId, reasons: [0] });

  const reply = await pending;
  ok(reply.text() === "pong", "the reply is returned");
  await client.close();
}

// --- request/reply needs 5.0 -----------------------------------------------

{
  const { client } = await connected(4);
  ok(client.protocolLevel === 4, "a forced connection is 3.1.1");
  let error = null;
  try {
    await client.request("a", "b");
  } catch (e) {
    error = e;
  }
  ok(error !== null && error.code === MqttErrorCode.Unsupported, "request on 3.1.1 is refused");
  await client.close();
}

// --- close ends everything -------------------------------------------------

{
  const { broker, client } = await connected();
  const { subscription } = await subscribe(client, broker, "a/b");
  const closing = client.close();
  ok(subscription.closed, "close ends the subscription at once");
  const disconnect = await broker.next();
  ok(disconnect.type === PacketType.Disconnect, "close sends a DISCONNECT");
  await closing;
}

// --- Message helpers -------------------------------------------------------

{
  const message = new Message({
    type: PacketType.Publish,
    topic: "t",
    payload: ENCODER.encode(JSON.stringify({ a: 1 })),
    qos: 0,
    retain: false,
    dup: false,
  });
  ok(message.json().a === 1, "Message.json() parses the payload");
  ok(message.text() === '{"a":1}', "Message.text() decodes the payload");
  ok(message.correlationData === null, "with no correlation data by default");
}

exit(report("client") > 0 ? 1 : 0);
