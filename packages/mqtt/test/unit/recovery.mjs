// Reconnecting, resuming a session as MQTT 5.0 section 4.4 says, and inbound
// flow control by acknowledging on ownership (DECISIONS D178) — against
// scripted brokers that can be dropped and refused at will.
import { exit } from "runtime:process";
import { MqttClient } from "../../dist/client.js";
import { MqttError, MqttErrorCode } from "../../dist/errors.js";
import { PacketType } from "../../dist/protocol/packets.js";
import { ok, report } from "./assert.mjs";
import { fakeBroker } from "./broker.mjs";

const ENCODER = new TextEncoder();
const tick = (ms = 10) => new Promise((resolve) => setTimeout(resolve, ms));
const FAST = { initialDelay: 5, maxDelay: 20 };

async function until(predicate, what, timeout = 2000) {
  const deadline = Date.now() + timeout;
  while (!predicate()) {
    if (Date.now() > deadline) throw new Error(`waited too long for ${what}`);
    await tick(2);
  }
}

/**
 * A dialer over scripted brokers. Each dial gets a fresh broker; `connack` is
 * what the next one answers with; `refuse` fails that many dials outright.
 */
function cluster(level = 5) {
  const brokers = [];
  const connects = [];
  const state = {
    refuse: 0,
    dials: 0,
    // Dials whose socket opens but whose broker vanishes before the CONNACK.
    dropHandshake: 0,
    connack: { sessionPresent: false, reasonCode: 0 },
  };
  const dial = async () => {
    state.dials++;
    if (state.refuse > 0) {
      state.refuse--;
      throw new Error("connection refused");
    }
    const broker = fakeBroker(level);
    brokers.push(broker);
    const connack = state.connack;
    const drop = state.dropHandshake > 0;
    if (drop) state.dropHandshake--;
    void (async () => {
      const connect = await broker.next();
      if (drop) {
        broker.close();
        return;
      }
      connects.push(connect);
      broker.send({ type: PacketType.Connack, ...connack });
    })().catch(() => {});
    return broker.transport;
  };
  return { dial, brokers, connects, state };
}

const options = (c, extra = {}) => ({
  dial: c.dial,
  clientId: "c",
  keepAlive: 0,
  reconnect: FAST,
  ...extra,
});

async function subscribed(client, broker, filter, extra = {}) {
  const pending = client.subscribe(filter, extra);
  const packet = await broker.next();
  broker.send({ type: PacketType.Suback, packetId: packet.packetId, reasons: [extra.qos ?? 0] });
  return pending;
}

function deliver(broker, packetId, text, qos = 1, topic = "jobs/a") {
  broker.send({
    type: PacketType.Publish,
    topic,
    payload: ENCODER.encode(text),
    qos,
    retain: false,
    dup: false,
    ...(qos > 0 ? { packetId } : {}),
  });
}

// -- acknowledging on ownership ------------------------------------------------

{
  // A QoS 1 message is acknowledged when the application takes it, so the
  // broker's quota — and the client's memory — is bounded by what is read.
  const c = cluster();
  const client = await MqttClient.connect("mqtt://test/", options(c));
  const broker = c.brokers[0];
  const subscription = await subscribed(client, broker, "jobs/#", { qos: 1 });
  deliver(broker, 1, "one");
  deliver(broker, 2, "two");
  await tick();
  // Nothing acknowledged yet: the next packet the broker sees is the PUBACK
  // for whatever is read first, and only then.
  const first = await subscription.next();
  ok(first.value.text() === "one", "the first message is read");
  const ack = await broker.next();
  ok(ack.type === PacketType.Puback && ack.packetId === 1, "and only it is acknowledged");
  await subscription.next();
  const ack2 = await broker.next();
  ok(ack2.packetId === 2, "the second is acknowledged when it is read");
  await client.close();
}

{
  // Matched by two subscriptions: acknowledged once both have taken it; one
  // ending with it unread counts as taking it.
  const c = cluster();
  const client = await MqttClient.connect("mqtt://test/", options(c));
  const broker = c.brokers[0];
  const a = await subscribed(client, broker, "jobs/#", { qos: 1 });
  const b = await subscribed(client, broker, "jobs/a", { qos: 1 });
  deliver(broker, 5, "shared");
  await tick();
  await a.next();
  let early = null;
  const sniff = broker.next().then((p) => {
    early = p;
  });
  await tick(30);
  ok(early === null, "not acknowledged while one of two subscriptions has it unread");
  const unsubscribing = b.unsubscribe();
  await sniff;
  ok(early?.type === PacketType.Unsubscribe, "ending the other unsubscribes its filter");
  c.brokers[0].send({ type: PacketType.Unsuback, packetId: early.packetId, reasons: [0] });
  const ack = await broker.next();
  ok(
    ack.type === PacketType.Puback && ack.packetId === 5,
    "and the message it never read is acknowledged as it ends",
  );
  await unsubscribing;
  await client.close();
}

{
  // QoS 0 cannot be flow-controlled: a subscription keeps at most maxBuffered
  // unread and drops the oldest.
  const c = cluster();
  const client = await MqttClient.connect("mqtt://test/", options(c));
  const broker = c.brokers[0];
  const subscription = await subscribed(client, broker, "t/#", { maxBuffered: 2 });
  for (let i = 1; i <= 5; i++) deliver(broker, 0, `m${i}`, 0, "t/x");
  await tick();
  ok(subscription.dropped === 3, "the QoS 0 messages past maxBuffered are dropped and counted");
  ok((await subscription.next()).value.text() === "m4", "the oldest go first");
  ok((await subscription.next()).value.text() === "m5", "the newest are kept");
  await client.close();
}

// -- opening ----------------------------------------------------------------

{
  const c = cluster();
  c.state.refuse = 2;
  const client = await MqttClient.connect("mqtt://test/", options(c, { timeout: 2000 }));
  ok(
    client.connected && c.state.dials === 3,
    "the first connection is retried past transient failures",
  );
  await client.close();
}

{
  const c = cluster();
  c.state.connack = { sessionPresent: false, reasonCode: 0x86 };
  let failure = null;
  await MqttClient.connect("mqtt://test/", options(c, { timeout: 2000 })).catch((e) => {
    failure = e;
  });
  ok(failure?.code === MqttErrorCode.Auth, "refused credentials fail the first connection at once");
  ok(c.state.dials <= 2, "without retrying (one dial, and the 3.1.1 fallback at most)");
}

{
  // A socket runtime:net refused for a reason of its own — an untrusted
  // certificate, a host outside --allow-net — is not retried until the timeout.
  for (const code of ["ERR_TLS", "ERR_CAPABILITY_DENIED"]) {
    let dials = 0;
    const dial = async () => {
      dials++;
      throw new MqttError("the connection could not be opened", {
        code: MqttErrorCode.Connection,
        cause: Object.assign(new TypeError("refused"), { code }),
      });
    };
    const started = Date.now();
    let failure = null;
    await MqttClient.connect("mqtt://test/", {
      dial,
      clientId: "c",
      keepAlive: 0,
      timeout: 5000,
    }).catch((e) => {
      failure = e;
    });
    ok(
      failure !== null && Date.now() - started < 1000,
      `${code} fails the first connection at once`,
    );
    ok(dials === 1, `after one dial (${code})`);
  }
}

// -- a fresh session after the drop -------------------------------------------

{
  const c = cluster();
  const client = await MqttClient.connect("mqtt://test/", options(c));
  const events = [];
  client.onDisconnect = (e) => events.push(`down ${e.code}`);
  client.onReconnect = () => events.push("up");
  const broker1 = c.brokers[0];
  const subscription = await subscribed(client, broker1, "jobs/#", { qos: 1 });

  // A QoS 1 publish is in flight when the socket goes.
  let rejected = null;
  const publishing = client.publish("out", "x", { qos: 1 }).catch((e) => {
    rejected = e;
  });
  await broker1.next();
  broker1.close();
  await until(() => events.includes("up"), "the reconnect");
  ok(events[0]?.startsWith("down "), "the drop is reported");
  ok(c.connects[1]?.cleanStart === true, "with no session expiry it reconnects clean");
  const resubscribe = await c.brokers[1].next();
  ok(
    resubscribe.type === PacketType.Subscribe && resubscribe.subscriptions[0].filter === "jobs/#",
    "the subscription is sent again on a fresh session",
  );
  ok(resubscribe.subscriptions[0].qos === 1, "with its options");
  c.brokers[1].send({ type: PacketType.Suback, packetId: resubscribe.packetId, reasons: [1] });
  await publishing;
  ok(
    rejected?.code === MqttErrorCode.Closed,
    "the in-flight publish rejects: 4.4 forbids resending it",
  );
  ok(!client.closed && client.failure === null, "and the client carries on");

  deliver(c.brokers[1], 1, "after");
  const next = await subscription.next();
  ok(next.value.text() === "after", "the same subscription receives on the new connection");
  await client.close();
}

// -- a resumed session ---------------------------------------------------------

{
  const c = cluster();
  c.state.connack = {
    sessionPresent: false,
    reasonCode: 0,
    properties: { assignedClientIdentifier: "assigned-7" },
  };
  const client = await MqttClient.connect(
    "mqtt://test/",
    options(c, { clientId: "", sessionExpiryInterval: 60 }),
  );
  const broker1 = c.brokers[0];
  const subscription = await subscribed(client, broker1, "jobs/#", { qos: 1 });
  // One QoS 1 message arrives and is never read before the drop.
  deliver(broker1, 9, "unread");
  // A QoS 1 publish waits on its PUBACK; a QoS 2 one has reached PUBREL.
  const p1 = client.publish("out", "one", { qos: 1 });
  const sent1 = await broker1.next();
  const p2 = client.publish("out", "two", { qos: 2 });
  const sent2 = await broker1.next();
  broker1.send({ type: PacketType.Pubrec, packetId: sent2.packetId, reasonCode: 0 });
  const rel = await broker1.next();
  ok(rel.type === PacketType.Pubrel, "the QoS 2 publish has reached PUBREL");

  c.state.connack = { sessionPresent: true, reasonCode: 0 };
  broker1.close();
  await until(() => c.brokers[1] !== undefined && c.connects[1] !== undefined, "the reconnect");
  ok(
    c.connects[1].cleanStart === false,
    "a session that can survive is resumed with Clean Start 0",
  );
  ok(c.connects[1].clientId === "assigned-7", "under the identifier the broker assigned");
  const resent1 = await c.brokers[1].next();
  ok(
    resent1.type === PacketType.Publish &&
      resent1.dup === true &&
      resent1.packetId === sent1.packetId,
    "the unacknowledged PUBLISH is resent with DUP and its original identifier (4.4)",
  );
  const resent2 = await c.brokers[1].next();
  ok(
    resent2.type === PacketType.Pubrel && resent2.packetId === sent2.packetId,
    "and the PUBREL, not the PUBLISH, for the QoS 2 one past PUBREC",
  );
  c.brokers[1].send({ type: PacketType.Puback, packetId: sent1.packetId, reasonCode: 0 });
  c.brokers[1].send({ type: PacketType.Pubcomp, packetId: sent2.packetId, reasonCode: 0 });
  await Promise.all([p1, p2]);
  ok(true, "both publishes complete on the resumed session");

  // The unread copy was discarded; the broker's redelivery is what is read.
  deliver(c.brokers[1], 9, "redelivered");
  const next = await subscription.next();
  ok(
    next.value.text() === "redelivered",
    "an unread QoS 1 copy is replaced by the broker's resend",
  );
  await client.close();
}

// -- waiting, and ending -------------------------------------------------------

{
  // A publish made while disconnected waits for the reconnect.
  const c = cluster();
  const client = await MqttClient.connect("mqtt://test/", options(c));
  c.state.refuse = 2;
  c.brokers[0].close();
  await until(() => !client.connected, "the drop");
  const publishing = client.publish("t", "held");
  // The CONNECT is the cluster's to read; reading before it has would race it.
  await until(() => c.connects[1] !== undefined, "the reconnect");
  const sent = await c.brokers[1].next();
  ok(
    sent.type === PacketType.Publish && sent.topic === "t",
    "a publish made while down is sent after",
  );
  await publishing;

  // And past its timeout it rejects.
  c.state.refuse = 1000;
  c.brokers[1].close();
  await until(() => !client.connected, "the second drop");
  let failure = null;
  await client.publish("t", "late", { timeout: 40 }).catch((e) => {
    failure = e;
  });
  ok(failure?.code === MqttErrorCode.Timeout, "one that outwaits its timeout rejects");
  const dials = c.state.dials;
  await client.close();
  await tick(60);
  ok(c.state.dials === dials, "a closed client stops dialling");
}

{
  // Session taken over: another client holds the id; reconnecting would fight it.
  const c = cluster();
  const client = await MqttClient.connect("mqtt://test/", options(c));
  c.brokers[0].send({ type: PacketType.Disconnect, reasonCode: 0x8e });
  await until(() => client.closed, "the end");
  ok(client.failure?.reasonCode === 0x8e, "Session taken over ends the client");
  await tick(40);
  ok(c.state.dials === 1, "without a reconnect");
}

{
  // Behind a proxy, an attempt the client abandoned can still reach the broker
  // after a later one connected, and take the session over. That is our own
  // ghost, not another client: reconnect instead of ending.
  const c = cluster();
  const client = await MqttClient.connect("mqtt://test/", options(c));
  let reconnects = 0;
  client.onReconnect = () => reconnects++;
  c.state.dropHandshake = 1;
  c.brokers[0].close();
  await until(() => reconnects === 1, "the reconnect past the dropped handshake");
  const live = c.brokers[c.brokers.length - 1];
  live.send({ type: PacketType.Disconnect, reasonCode: 0x8e });
  await until(() => reconnects === 2, "a reconnect after the ghost's takeover");
  ok(!client.closed, "a takeover by our own abandoned attempt does not end the client");
  // With no ghost left, a second takeover is another client's, and final.
  c.brokers[c.brokers.length - 1].send({ type: PacketType.Disconnect, reasonCode: 0x8e });
  await until(() => client.closed, "the end");
  ok(client.failure?.reasonCode === 0x8e, "but one with no abandoned attempt behind it does");
}

{
  const c = cluster();
  const client = await MqttClient.connect("mqtt://test/", options(c, { reconnect: false }));
  c.brokers[0].close();
  await until(() => client.closed, "the end");
  ok(
    client.failure?.code === MqttErrorCode.Connection,
    "reconnect: false ends the client on a drop",
  );
}

exit(report("recovery") > 0 ? 1 : 0);
