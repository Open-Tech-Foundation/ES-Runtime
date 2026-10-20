// The client keeping to what the broker said it allows (MQTT 5.0 CONNACK
// limits), and not losing a message that arrives before anything subscribed.
import { exit } from "runtime:process";
import { MqttClient } from "../../dist/client.js";
import { MqttErrorCode } from "../../dist/errors.js";
import { PacketType } from "../../dist/protocol/packets.js";
import { ok, report } from "./assert.mjs";
import { fakeBroker } from "./broker.mjs";

const ENCODER = new TextEncoder();
const tick = (ms = 10) => new Promise((resolve) => setTimeout(resolve, ms));

/** A client over a scripted broker whose CONNACK carries `properties`. */
async function connected(properties, options = {}, before = () => {}) {
  const broker = fakeBroker(5);
  // Counts what the client wrote, so a test can check that nothing was sent
  // without a second reader racing the broker's own.
  const counter = { writes: 0 };
  const transport = {
    read: () => broker.transport.read(),
    write: (bytes) => {
      counter.writes++;
      return broker.transport.write(bytes);
    },
    close: () => broker.transport.close(),
  };
  const pending = MqttClient.over(transport, { clientId: "c", keepAlive: 0, ...options }, 5);
  await broker.next();
  broker.send({ type: PacketType.Connack, sessionPresent: true, reasonCode: 0, properties });
  before(broker);
  const client = await pending;
  return { broker, client, counter };
}

async function failureOf(promise) {
  try {
    await promise;
    return null;
  } catch (e) {
    return e;
  }
}

// --- Maximum QoS and Retain Available ------------------------------------------

{
  const { client } = await connected({ maximumQoS: 1, retainAvailable: 0 });
  const qos2 = await failureOf(client.publish("t", "x", { qos: 2 }));
  ok(
    qos2?.code === MqttErrorCode.Unsupported,
    "a QoS above the broker's maximum is refused up front",
  );
  const retained = await failureOf(client.publish("t", "x", { retain: true }));
  ok(retained?.code === MqttErrorCode.Unsupported, "and so is retain when the broker has none");
  ok(client.failure === null, "without taking the connection down");
}

// --- Maximum Packet Size --------------------------------------------------------

{
  const { client } = await connected({ maximumPacketSize: 64 });
  const big = await failureOf(client.publish("t", "x".repeat(100)));
  ok(big?.code === MqttErrorCode.Unsupported, "a packet over the broker's maximum size is refused");
  ok(client.failure === null, "and the connection stands");
}

// --- subscription features -------------------------------------------------------

{
  const { client } = await connected({
    wildcardSubscriptionAvailable: 0,
    sharedSubscriptionAvailable: 0,
    subscriptionIdentifierAvailable: 0,
  });
  ok(
    (await failureOf(client.subscribe("a/+")))?.code === MqttErrorCode.Unsupported,
    "a wildcard filter is refused when the broker has no wildcards",
  );
  ok(
    (await failureOf(client.subscribe("$share/g/a")))?.code === MqttErrorCode.Unsupported,
    "a shared subscription is refused when the broker has none",
  );
  ok(
    (await failureOf(client.subscribe("a", { subscriptionIdentifier: 3 })))?.code ===
      MqttErrorCode.Unsupported,
    "a subscription identifier is refused when the broker has none",
  );
}

// --- Receive Maximum -------------------------------------------------------------

{
  // The broker takes two QoS 1 publishes unacknowledged at once; a third waits
  // for a slot rather than breaking the window.
  const { broker, client, counter } = await connected({ receiveMaximum: 2 });
  const done = [];
  const before = counter.writes;
  const publishes = [1, 2, 3].map((n) =>
    client.publish("t", String(n), { qos: 1 }).then(() => done.push(n)),
  );
  const first = await broker.next();
  const second = await broker.next();
  await tick(60);
  ok(counter.writes - before === 2, "a third publish is held while two are unacknowledged");
  broker.send({ type: PacketType.Puback, packetId: first.packetId, reasonCode: 0 });
  const third = await broker.next();
  ok(third?.type === PacketType.Publish, "and goes out when one is acknowledged");
  broker.send({ type: PacketType.Puback, packetId: second.packetId, reasonCode: 0 });
  broker.send({ type: PacketType.Puback, packetId: third.packetId, reasonCode: 0 });
  await Promise.all(publishes);
  ok(done.length === 3, "all three resolve");

  // A publish that cannot get a slot within its timeout rejects.
  const a = client.publish("t", "a", { qos: 1 });
  const b = client.publish("t", "b", { qos: 1 });
  await broker.next();
  await broker.next();
  const waited = await failureOf(client.publish("t", "c", { qos: 1, timeout: 40 }));
  ok(
    waited?.code === MqttErrorCode.Timeout,
    "a publish that waits past its timeout for a slot rejects",
  );
  void a.catch(() => {});
  void b.catch(() => {});
}

// --- packet identifiers in flight are not reused -----------------------------------

{
  const { broker, client } = await connected(undefined);
  const held = client.publish("t", "held", { qos: 1 });
  const first = await broker.next();
  // Burn through the whole identifier space; none may collide with `first`.
  const seen = new Set();
  for (let i = 0; i < 65534; i++) {
    const p = client.publish("t", "x", { qos: 1 });
    const packet = await broker.next();
    seen.add(packet.packetId);
    broker.send({ type: PacketType.Puback, packetId: packet.packetId, reasonCode: 0 });
    await p;
  }
  ok(!seen.has(first.packetId), "an identifier still in flight is skipped when the counter wraps");
  broker.send({ type: PacketType.Puback, packetId: first.packetId, reasonCode: 0 });
  await held;
}

// --- messages before anything subscribed -------------------------------------------

{
  // A persistent session's backlog arrives straight after the CONNACK, before
  // the client object exists, let alone a subscription.
  const unmatched = [];
  const { client } = await connected(
    undefined,
    { cleanStart: false, onUnmatchedMessage: (m) => unmatched.push(m.text()) },
    (broker) => {
      for (const text of ["queued 1", "queued 2"]) {
        broker.send({
          type: PacketType.Publish,
          topic: "jobs/a",
          payload: ENCODER.encode(text),
          qos: 0,
          retain: false,
          dup: false,
        });
      }
    },
  );
  await tick(20);
  ok(unmatched.join(",") === "queued 1,queued 2", "a session's backlog reaches onUnmatchedMessage");
  ok(client.sessionPresent, "on a resumed session");
}

// --- one filter, two subscriptions -----------------------------------------------------

{
  const { broker, client, counter } = await connected(undefined);
  const subscribe = async () => {
    const pending = client.subscribe("a/b");
    const packet = await broker.next();
    broker.send({ type: PacketType.Suback, packetId: packet.packetId, reasons: [0] });
    return pending;
  };
  const one = await subscribe();
  const two = await subscribe();
  const before = counter.writes;
  await one.unsubscribe();
  await tick(40);
  ok(counter.writes === before, "ending one of two subscriptions to a filter sends no UNSUBSCRIBE");
  ok(!two.closed, "and the other keeps receiving");
  const ending = two.unsubscribe();
  const unsub = await broker.next();
  ok(unsub.type === PacketType.Unsubscribe, "the last one unsubscribes at the broker");
  broker.send({ type: PacketType.Unsuback, packetId: unsub.packetId, reasons: [0] });
  await ending;
}

exit(report("compliance") > 0 ? 1 : 0);
