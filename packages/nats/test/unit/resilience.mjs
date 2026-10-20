// What the server's -ERR table, the cluster's server list and a slow consumer
// ask of the client: recoverable errors that do not end it, unrecoverable ones
// that it recovers from, TLS that is never downgraded, close() that flushes,
// limits that hold, and the hooks that say what happened.
import { exit } from "runtime:process";
import { NatsClient, NatsErrorCode, openWithDial } from "../../dist/index.js";
import { createInbox } from "../../dist/protocol/subject.js";
import { is, ok, report } from "./assert.mjs";
import { fakeBroker } from "./broker.mjs";

const TEXT = new TextDecoder();
const tick = (ms = 10) => new Promise((resolve) => setTimeout(resolve, ms));

async function until(predicate, what, timeout = 2000) {
  const deadline = Date.now() + timeout;
  while (!predicate()) {
    if (Date.now() > deadline) throw new Error(`waited too long for ${what}`);
    await tick(2);
  }
}

/**
 * A dialer that hands each dial a fresh broker, accepting with `info`. With
 * `seedOnce`, the seed host refuses every dial after the first, so a reconnect
 * has to use what the cluster advertised.
 */
function cluster(info = () => ({}), seedOnce = false) {
  const brokers = [];
  const dials = [];
  const dial = async (address) => {
    if (seedOnce && address.hostname === "seed" && brokers.length > 0) {
      throw new Error("connection refused");
    }
    dials.push(address);
    const broker = fakeBroker();
    brokers.push(broker);
    void broker.accept(info(brokers.length - 1)).catch(() => {});
    return broker.transport;
  };
  return { dial, brokers, dials };
}

// -- recoverable -ERRs do not end the client ----------------------------------

{
  const broker = fakeBroker();
  const pending = NatsClient.over(broker.transport, { reconnect: false, pingInterval: 0 });
  await broker.accept();
  const nc = await pending;
  const errors = [];
  nc.onError = (e) => errors.push(e);
  const refused = nc.subscribe("secret.data");
  const kept = nc.subscribe("public.data");
  await broker.next();
  await broker.next();

  broker.sendErr('Permissions Violation for Subscription to "secret.data"');
  await tick(20);
  ok(!nc.closed && nc.connected, "a permissions violation leaves the client connected");
  ok(refused.closed, "the refused subscription ends");
  is(errors[0]?.code, NatsErrorCode.Server, "and the error is reported through onError");
  let reason = null;
  await refused.nextMessage(10).catch((e) => {
    reason = e;
  });
  ok(reason?.serverError?.startsWith("Permissions Violation"), "carrying the server's words");
  const unsub = await broker.next();
  is(unsub.kind, "unsub", "it is unsubscribed, so a reconnect does not re-send it");
  ok(!kept.closed, "the other subscription carries on");

  broker.sendErr('Permissions Violation for Publish to "secret.out"');
  await tick(20);
  ok(!nc.closed && errors.length === 2, "a publish violation is reported, not fatal");
  broker.sendMsg("public.data", kept.sid, "still here");
  is((await kept.nextMessage(500)).text(), "still here", "messages keep arriving");
  await nc.close();
}

// -- unrecoverable -ERRs are recovered from -------------------------------------

{
  // The server sends Stale Connection and closes the socket: a lost connection,
  // reconnected like any other, not the end of the client.
  const c = cluster();
  let reconnects = 0;
  const connection = await openWithDial(
    "nats://h:4222",
    { reconnect: { maxAttempts: 5, wait: 1, jitter: 0 }, pingInterval: 0 },
    { onReconnect: () => reconnects++ },
    c.dial,
  );
  c.brokers[0].sendErr("Stale Connection");
  c.brokers[0].close();
  await until(() => reconnects === 1, "the reconnect");
  ok(connection.connected && !connection.closed, "Stale Connection is reconnected, not fatal");
  await connection.close();
}

// -- TLS is never downgraded -----------------------------------------------------

{
  // A tls:// client learns plain host:port peers from connect_urls — which
  // carry no TLS information. A reconnect to one must still be TLS.
  const c = cluster((i) => (i === 0 ? { connect_urls: ["10.0.0.9:4222"] } : {}), true);
  const tlsDial = async (address) => c.dial(address);
  let reconnects = 0;
  const connection = await openWithDial(
    "tls://seed:4222",
    { reconnect: { maxAttempts: 5, wait: 1, jitter: 0 }, pingInterval: 0 },
    { onReconnect: () => reconnects++ },
    tlsDial,
  );
  ok(c.brokers[0].tlsUpgrades === 1, "the seed connection is TLS");
  c.brokers[0].close();
  await until(() => reconnects === 1, "the reconnect");
  const peer = c.dials.find((d) => d.hostname === "10.0.0.9");
  ok(peer?.tls === true, "a discovered peer is dialled with TLS");
  const reached = c.brokers[c.dials.indexOf(peer)];
  ok(reached?.tlsUpgrades === 1, "and upgraded, never spoken to in the clear");
  await connection.close();
}

{
  // An IPv6 peer in connect_urls is dialled without its brackets.
  const c = cluster((i) => (i === 0 ? { connect_urls: ["[fd00::7]:4333"] } : {}), true);
  let reconnects = 0;
  const connection = await openWithDial(
    "nats://seed:4222",
    { reconnect: { maxAttempts: 5, wait: 1, jitter: 0 }, pingInterval: 0 },
    { onReconnect: () => reconnects++ },
    c.dial,
  );
  c.brokers[0].close();
  await until(() => reconnects === 1, "the reconnect");
  ok(
    c.dials.some((d) => d.hostname === "fd00::7" && d.port === 4333),
    "an IPv6 peer is dialled as fd00::7, port 4333",
  );
  await connection.close();
}

// -- close() flushes what was queued ------------------------------------------------

{
  // Publishes the caller did not await are still sent before the socket goes.
  const broker = fakeBroker();
  let gate = null;
  const transport = {
    ...broker.transport,
    // Writes are held until released, so close() runs with them queued.
    write: (bytes) =>
      new Promise((resolve, reject) => {
        const go = () => broker.transport.write(bytes).then(resolve, reject);
        if (gate === null) go();
        else gate.push(go);
      }),
  };
  const pending = NatsClient.over(transport, { reconnect: false, pingInterval: 0 });
  await broker.accept();
  const nc = await pending;
  gate = [];
  for (let i = 0; i < 3; i++) void nc.publish("out", `m${i}`).catch(() => {});
  const closing = nc.close();
  await tick(20);
  for (const go of gate.splice(0)) go();
  gate = null;
  const got = [];
  for (let i = 0; i < 3; i++) {
    const command = await broker.next();
    if (command?.kind === "pub") got.push(TEXT.decode(command.payload));
  }
  await closing;
  is(got, ["m0", "m1", "m2"], "close() sends queued publishes before closing the socket");
}

// -- auto-unsubscribe survives a reconnect ---------------------------------------------

{
  const c = cluster();
  let reconnects = 0;
  const connection = await openWithDial(
    "nats://h:4222",
    { reconnect: { maxAttempts: 5, wait: 1, jitter: 0 }, pingInterval: 0 },
    { onReconnect: () => reconnects++ },
    c.dial,
  );
  const sid = connection.subscribe("jobs");
  connection.unsubscribe(sid, 3);
  await c.brokers[0].next();
  await c.brokers[0].next();
  c.brokers[0].sendMsg("jobs", sid, "one");
  await tick(10);
  c.brokers[0].close();
  await until(() => reconnects === 1, "the reconnect");
  const sub = await c.brokers[1].next();
  const unsub = await c.brokers[1].next();
  ok(sub.kind === "sub" && sub.sid === sid, "a limited subscription is re-sent");
  ok(unsub.kind === "unsub" && unsub.max === 2, "with what is left of its limit (2 of 3)");
  await connection.close();
}

{
  // The client's max option ends the subscription after that many messages.
  const broker = fakeBroker();
  const pending = NatsClient.over(broker.transport, { reconnect: false, pingInterval: 0 });
  await broker.accept();
  const nc = await pending;
  const subscription = nc.subscribe("two", { max: 2 });
  await broker.next();
  const unsub = await broker.next();
  ok(unsub.kind === "unsub" && unsub.max === 2, "max tells the server too");
  broker.sendMsg("two", subscription.sid, "a");
  broker.sendMsg("two", subscription.sid, "b");
  const read = [];
  for await (const message of subscription) read.push(message.text());
  is(read, ["a", "b"], "both messages are read, then the iteration ends");
  await nc.close();
}

// -- a slow consumer is bounded --------------------------------------------------------

{
  const broker = fakeBroker();
  const pending = NatsClient.over(broker.transport, { reconnect: false, pingInterval: 0 });
  await broker.accept();
  const nc = await pending;
  const errors = [];
  nc.onError = (e) => errors.push(e);
  const subscription = nc.subscribe("flood", { pendingLimits: { messages: 3 } });
  await broker.next();
  for (let i = 0; i < 10; i++) broker.sendMsg("flood", subscription.sid, `m${i}`);
  await tick(30);
  is(subscription.dropped, 7, "messages past the pending limit are dropped and counted");
  is(errors.length, 1, "the slow consumer is reported once per run");
  is(errors[0]?.code, NatsErrorCode.SlowConsumer, "as ERR_NATS_SLOW_CONSUMER");
  is((await subscription.nextMessage(100)).text(), "m0", "the oldest are kept");
  await nc.close();
}

// -- a large message in many chunks ---------------------------------------------------

{
  const broker = fakeBroker();
  const pending = NatsClient.over(broker.transport, { reconnect: false, pingInterval: 0 });
  await broker.accept();
  const nc = await pending;
  const subscription = nc.subscribe("big");
  await broker.next();
  const payload = new Uint8Array(2 * 1024 * 1024).map((_, i) => i & 0xff);
  const ENC = new TextEncoder();
  const head = ENC.encode(`MSG big ${subscription.sid} ${payload.length}\r\n`);
  const tail = ENC.encode(`\r\nMSG big ${subscription.sid} 5\r\nsmall\r\n`);
  const all = new Uint8Array(head.length + payload.length + tail.length);
  all.set(head);
  all.set(payload, head.length);
  all.set(tail, head.length + payload.length);
  const started = Date.now();
  for (let at = 0; at < all.length; at += 1024) broker.sendRaw(all.slice(at, at + 1024));
  const first = await subscription.nextMessage(5000);
  const second = await subscription.nextMessage(5000);
  is(first.payload.length, payload.length, "a 2 MB message in 2 048 chunks arrives whole");
  ok(first.payload[123456] === (123456 & 0xff), "and intact");
  is(second.text(), "small", "the message sharing its last chunk is read too");
  ok(Date.now() - started < 3000, "in linear time");
  await nc.close();
}

// -- the hooks, and the inbox ------------------------------------------------------------

{
  const c = cluster();
  const events = [];
  // Built through the connection seam, then wrapped the way connect() wraps.
  const connection = await openWithDial(
    "nats://h:4222",
    { reconnect: { maxAttempts: 5, wait: 1, jitter: 0 }, pingInterval: 0 },
    {
      onDisconnect: () => events.push("down"),
      onReconnect: () => events.push("up"),
    },
    c.dial,
  );
  c.brokers[c.brokers.length - 1].close();
  await until(() => events.includes("up"), "the reconnect");
  is(events, ["down", "up"], "the connection reports its drop and its recovery");
  await connection.close();
}

{
  const inboxes = new Set();
  for (let i = 0; i < 1000; i++) inboxes.add(createInbox());
  is(inboxes.size, 1000, "inboxes do not repeat");
  ok(
    [...inboxes].every((one) => /^_INBOX\.[A-Za-z0-9]{22}$/.test(one)),
    "and keep their shape",
  );
}

exit(report("resilience") > 0 ? 1 : 0);
