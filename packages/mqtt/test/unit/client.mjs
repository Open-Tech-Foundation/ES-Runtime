// The client: publish at each QoS, subscribe and route by filter, request/reply
// over the 5.0 convention, and unsubscribe — all against the scripted broker.
import { exit } from "runtime:process";
import { Message, MqttClient } from "../../dist/client.js";
import { MqttErrorCode } from "../../dist/errors.js";
import { PacketType } from "../../dist/protocol/packets.js";
import { ok, report, throws } from "./assert.mjs";
import { fakeBroker } from "./broker.mjs";

const ENCODER = new TextEncoder();

async function connected(level = 5, options = {}) {
  const broker = fakeBroker(level);
  const pending = MqttClient.over(
    broker.transport,
    { clientId: "c", keepAlive: 0, ...options },
    level,
  );
  const connect = await broker.next();
  ok(connect.type === PacketType.Connect, "the client sends a CONNECT");
  broker.send({ type: PacketType.Connack, sessionPresent: false, reasonCode: 0 });
  const client = await pending;
  return { broker, client };
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
