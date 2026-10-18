// The client against a real Mosquitto.
//
// Everything the unit suites cannot show is here: that a broker accepts the
// CONNECT this client writes, that a publish at each QoS is acknowledged by
// Mosquitto itself, that the two wildcards route the way the specification says,
// that a retained message is kept and replayed, that request/reply works over
// the 5.0 response-topic convention, that a will fires when a client vanishes
// without a DISCONNECT, and that a persistent session is resumed. The framing
// either matches Mosquitto's or nothing below the first assertion passes.
//
//   eval "$(test/mosquitto-server.sh)" && test/run.sh
//   # or point MQTT_URL at a broker you already have
import { env, exit } from "runtime:process";
import { Message, MqttClient } from "../dist/client.js";
import { Connection, parseMqttUrl } from "../dist/connection.js";
import { netTransport } from "../dist/transport.js";
import { is, ok, report } from "./unit/assert.mjs";

const url = env.MQTT_URL;
if (url === undefined) {
  console.log("skip e2e — run test/mosquitto-server.sh first (it prints MQTT_URL)");
  exit(0);
}

const ENCODER = new TextEncoder();
const DECODER = new TextDecoder();
const suffix = Math.random().toString(36).slice(2, 8);
const topic = (path) => `esrun/e2e/${suffix}/${path}`;

/** A fresh client, with a unique client identifier so runs do not collide. */
async function client(settings = {}) {
  return MqttClient.connect(url, {
    clientId: `esrun-e2e-${suffix}-${Math.random().toString(36).slice(2, 8)}`,
    keepAlive: 30,
    ...settings,
  });
}

/** Collect up to `count` messages, giving up after `timeout` ms. */
async function collect(subscription, count, timeout = 5000) {
  const out = [];
  const deadline = Date.now() + timeout;
  while (out.length < count) {
    const remaining = deadline - Date.now();
    if (remaining <= 0) break;
    try {
      out.push(await subscription.nextMessage(remaining));
    } catch {
      break;
    }
  }
  return out;
}

// -- the handshake ----------------------------------------------------------

{
  const c = await client();
  is(c.protocolLevel, 5, "the handshake negotiates MQTT 5.0");
  is(c.sessionPresent, false, "a clean start begins with no session");
  ok(c.failure === null, "the connection is healthy");
  await c.ping();
  ok(true, "a PINGREQ draws a PINGRESP from a real broker");
  await c.close();
  ok(c.closed, "close ends the client");
}

// -- publish and subscribe, at each QoS -------------------------------------

{
  const c = await client();
  const sub = await c.subscribe(topic("qos/#"), { qos: 2 });
  is(sub.grantedQos, 2, "the broker grants the requested QoS");

  await c.publish(topic("qos/zero"), "z", { qos: 0 });
  await c.publish(topic("qos/one"), "o", { qos: 1 });
  await c.publish(topic("qos/two"), "t", { qos: 2 });

  const messages = await collect(sub, 3);
  is(messages.length, 3, "all three publishes arrive");
  is(
    messages
      .map((m) => m.topic.split("/").pop())
      .sort()
      .join(","),
    "one,two,zero",
    "on the topics they were published to",
  );
  ok(
    messages.every((m) => m instanceof Message),
    "each is a Message",
  );

  await c.close();
}

// -- wildcards --------------------------------------------------------------

{
  const c = await client();
  const sub = await c.subscribe(topic("wild/+/temp"), { qos: 0 });
  await c.publish(topic("wild/a/temp"), "match", { qos: 0 });
  await c.publish(topic("wild/a/humidity"), "no", { qos: 0 });
  await c.publish(topic("wild/a/b/temp"), "no", { qos: 0 });

  const messages = await collect(sub, 1);
  is(messages.length, 1, "only the filter's match arrives");
  is(messages[0].text(), "match", "and it is the right one");
  await c.close();
}

// -- retained messages ------------------------------------------------------

{
  const c = await client();
  const retained = topic("retained");
  await c.publish(retained, "kept", { qos: 1, retain: true });

  const sub = await c.subscribe(retained, { qos: 1 });
  const [message] = await collect(sub, 1);
  is(message?.text(), "kept", "a retained message is replayed on subscribe");
  is(message?.retain, true, "and is marked retained");

  await c.publish(retained, "", { qos: 1, retain: true }); // clear it
  await c.close();
}

// -- request / reply --------------------------------------------------------

{
  const responder = await client();
  const requester = await client();
  const service = await responder.subscribe(topic("echo"), { qos: 1 });

  const serving = (async () => {
    const [request] = await collect(service, 1);
    is(typeof request?.responseTopic, "string", "the request carries a response topic");
    is(request?.correlationData.length, 8, "and correlation data");
    await responder.publish(request.responseTopic, `echo:${request.text()}`, {
      qos: 0,
      correlationData: request.correlationData,
    });
  })();

  const reply = await requester.request(topic("echo"), "hi", { timeout: 5000 });
  is(reply.text(), "echo:hi", "the reply comes back over the response topic");
  await serving;
  await responder.close();
  await requester.close();
}

// -- unsubscribe ------------------------------------------------------------

{
  const c = await client();
  const sub = await c.subscribe(topic("drop"), { qos: 0 });
  await c.unsubscribe(topic("drop"));
  ok(sub.closed, "unsubscribe ends the subscription");
  await c.publish(topic("drop"), "after", { qos: 0 });
  const messages = await collect(sub, 1, 300);
  is(messages.length, 0, "and no message arrives afterwards");
  await c.close();
}

// -- the will fires on an ungraceful disconnect -----------------------------

{
  const watcher = await client();
  const sub = await watcher.subscribe(topic("will"), { qos: 1 });

  const { hostname, port, tls } = parseMqttUrl(url);
  const transport = await netTransport({ hostname, port, tls });
  await Connection.over(
    transport,
    {
      clientId: `esrun-e2e-will-${suffix}`,
      keepAlive: 30,
      will: { topic: topic("will"), payload: ENCODER.encode("gone"), qos: 1, retain: false },
    },
    {},
    5,
  );
  // Drop the socket without a DISCONNECT, as a crash would.
  await transport.close();

  const [will] = await collect(sub, 1, 8000);
  is(will?.text(), "gone", "the will is delivered when the client vanishes");
  await watcher.close();
}

// -- a persistent session is resumed ----------------------------------------

{
  const clientId = `esrun-e2e-session-${suffix}`;
  const first = await MqttClient.connect(url, {
    clientId,
    cleanStart: true,
    sessionExpiryInterval: 300,
    keepAlive: 30,
  });
  await first.subscribe(topic("session"), { qos: 1 });
  await first.close();

  const second = await MqttClient.connect(url, { clientId, cleanStart: false, keepAlive: 30 });
  is(second.sessionPresent, true, "the session is resumed after a graceful disconnect");
  await second.close();

  // Leave nothing behind.
  const cleanup = await MqttClient.connect(url, { clientId, cleanStart: true });
  await cleanup.close();
}

// -- optional TLS listener --------------------------------------------------

if (env.MQTT_TLS_URL !== undefined) {
  const ca = env.MQTT_CA;
  if (ca === undefined) throw new Error("MQTT_CA is required when MQTT_TLS_URL is set");

  const secure = await MqttClient.connect(env.MQTT_TLS_URL, {
    clientId: `esrun-e2e-tls-${suffix}`,
    keepAlive: 0,
    ca,
    servername: "localhost",
  });
  is(secure.protocolLevel, 5, "the TLS handshake reaches Mosquitto");
  await secure.close();

  let untrusted = null;
  try {
    await MqttClient.connect(env.MQTT_TLS_URL, {
      clientId: `esrun-e2e-tls-bad-${suffix}`,
      keepAlive: 0,
      ca: "not a trusted certificate authority",
    });
  } catch (e) {
    untrusted = e;
  }
  is(untrusted?.code, "ERR_MQTT_CONNECTION", "an untrusted TLS certificate is rejected");
}

ok(DECODER.decode(ENCODER.encode("x")) === "x", "sanity: text round-trips");

exit(report("mqtt e2e") > 0 ? 1 : 0);
