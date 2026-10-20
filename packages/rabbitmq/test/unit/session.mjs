// The session: reconnecting, re-running setup, and carrying channels and
// subscriptions across a lost connection (DECISIONS D177), against scripted
// brokers that can be dropped and refused at will.
import { exit } from "runtime:process";
import { RabbitError } from "../../dist/errors.js";
import { openSession } from "../../dist/session.js";
import { is, ok, report } from "./assert.mjs";
import { Broker, handshake, loopback, sendContent } from "./broker.mjs";

const enc = (text) => new TextEncoder().encode(text);
const dec = (bytes) => new TextDecoder().decode(bytes);
const settle = (ms = 10) => new Promise((resolve) => setTimeout(resolve, ms));
const FAST = { initialDelay: 5, maxDelay: 20 };

async function until(predicate, what, timeout = 2000) {
  const deadline = Date.now() + timeout;
  while (!predicate()) {
    if (Date.now() > deadline) throw new Error(`waited too long for ${what}`);
    await settle(2);
  }
}

/**
 * A broker that answers the routine methods by itself and records the rest, so
 * a test can drop it and look at what the client did next.
 */
const PILOTS = [];

function autopilot(broker) {
  const seen = [];
  const seq = new Map(); // channel -> next confirm tag, once confirm.select is seen
  let stopped = false;
  const run = async () => {
    while (!stopped) {
      let m;
      try {
        m = await broker.any(20);
      } catch {
        continue;
      }
      seen.push(m);
      const ch = m.channel;
      switch (m.name) {
        case "channel.open":
          await broker.send("channel.open-ok", {}, ch);
          break;
        case "confirm.select":
          seq.set(ch, 1n);
          await broker.send("confirm.select-ok", {}, ch);
          break;
        case "basic.qos":
          await broker.send("basic.qos-ok", {}, ch);
          break;
        case "queue.declare":
          await broker.send(
            "queue.declare-ok",
            { queue: m.args.queue || "amq.gen-1", messageCount: 0, consumerCount: 0 },
            ch,
          );
          break;
        case "basic.consume":
          await broker.send("basic.consume-ok", { consumerTag: m.args.consumerTag }, ch);
          break;
        case "basic.cancel":
          await broker.send("basic.cancel-ok", { consumerTag: m.args.consumerTag }, ch);
          break;
        case "channel.close":
          await broker.send("channel.close-ok", {}, ch);
          break;
        case "connection.close":
          await broker.send("connection.close-ok", {}, 0);
          break;
        case "basic.publish":
          if (seq.has(ch) && broker.autoConfirm !== false) {
            const tag = seq.get(ch);
            seq.set(ch, tag + 1n);
            await broker.send("basic.ack", { deliveryTag: tag, multiple: false }, ch);
          } else if (seq.has(ch)) {
            seq.set(ch, seq.get(ch) + 1n);
          }
          break;
        default:
          break;
      }
    }
  };
  void run();
  PILOTS.push(() => {
    stopped = true;
  });
  return {
    seen,
    names: () => seen.map((m) => m.name),
    stop: () => {
      stopped = true;
    },
  };
}

/** A dialer over scripted brokers; `refuse` fails the next N dials. */
function cluster() {
  const brokers = [];
  const pilots = [];
  const urls = [];
  const state = { refuse: 0, badPassword: false, dials: 0 };
  const dial = async (address) => {
    state.dials++;
    urls.push(`${address.hostname}:${address.port}`);
    if (state.refuse > 0) {
      state.refuse--;
      throw new Error("connection refused");
    }
    const pair = loopback();
    const broker = new Broker(pair.server);
    brokers.push(broker);
    if (state.badPassword) {
      void (async () => {
        await broker.awaitHeader();
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
          replyText: "ACCESS_REFUSED",
          classId: 10,
          methodId: 11,
        });
      })().catch(() => {});
    } else {
      void handshake(broker)
        .then(() => {
          pilots.push(autopilot(broker));
        })
        .catch(() => {});
    }
    return pair.client;
  };
  return { dial, brokers, pilots, urls, state };
}

const base = (c, extra = {}) => ({ dial: c.dial, heartbeat: 0, reconnect: FAST, ...extra });

// -- opening -----------------------------------------------------------------

{
  // A broker still starting refuses the first dials; the session waits it out.
  const c = cluster();
  c.state.refuse = 2;
  const errors = [];
  const session = await openSession("amqp://a/", base(c, { timeout: 2000 }));
  ok(session.connected, "the first connection is retried until it opens");
  is(c.state.dials, 3, "after two refused dials");
  await session.close();
  ok(errors.length === 0, "nothing is left over");
}

{
  // Refused credentials do not get better: the session fails at once.
  const c = cluster();
  c.state.badPassword = true;
  let failure = null;
  const started = Date.now();
  await openSession("amqp://a/", base(c, { timeout: 2000 })).catch((e) => {
    failure = e;
  });
  is(failure?.code, "ERR_RABBITMQ_AUTH", "an auth failure is not retried");
  is(c.state.dials, 1, "after a single dial");
  ok(Date.now() - started < 1000, "and without waiting out the timeout");
}

{
  // Nor does a socket runtime:net refused for a reason of its own — an
  // untrusted certificate, a host outside --allow-net.
  for (const code of ["ERR_TLS", "ERR_CAPABILITY_DENIED"]) {
    let dials = 0;
    const dial = async () => {
      dials++;
      throw new RabbitError("the connection could not be opened", {
        code: "ERR_RABBITMQ_CONNECTION",
        cause: Object.assign(new TypeError("refused"), { code }),
      });
    };
    const started = Date.now();
    let failure = null;
    await openSession("amqp://a/", { dial, heartbeat: 0, reconnect: FAST, timeout: 5000 }).catch(
      (e) => {
        failure = e;
      },
    );
    ok(failure !== null && Date.now() - started < 1000, `${code} fails the session at once`);
    is(dials, 1, `after one dial (${code})`);
  }
}

{
  // Nor does a URL that cannot be read.
  const c = cluster();
  let failure = null;
  await openSession("http://a/", base(c)).catch((e) => {
    failure = e;
  });
  ok(failure !== null, "an unreadable URL is refused");
  is(c.state.dials, 0, "before any dial");
}

{
  // Several URLs are tried in turn.
  const c = cluster();
  c.state.refuse = 1;
  const session = await openSession(["amqp://one:5672/", "amqp://two:5672/"], base(c));
  is(c.urls, ["one:5672", "two:5672"], "a refused URL moves on to the next");
  await session.close();
}

// -- recovering ----------------------------------------------------------------

{
  const c = cluster();
  const session = await openSession("amqp://a/", base(c));
  const events = [];
  session.onDisconnect = (e) => events.push(`down ${e.code}`);
  session.onConnect = () => events.push("up");

  let setups = 0;
  const jobs = await session.channel({
    confirm: true,
    setup: async (channel) => {
      setups++;
      await channel.queueDeclare("jobs", { durable: true });
    },
  });
  is(setups, 1, "setup runs on the first open");

  const subscription = jobs.subscribe("jobs", { prefetch: 4 });
  const iterator = subscription[Symbol.asyncIterator]();
  await until(() => c.pilots[0]?.names().includes("basic.consume"), "the first consume");
  const consume1 = c.pilots[0].seen.find((m) => m.name === "basic.consume");
  const tag1 = consume1.args.consumerTag;
  const chan1 = consume1.channel;

  await sendContent(
    c.brokers[0],
    "basic.deliver",
    { consumerTag: tag1, deliveryTag: 1n, redelivered: false, exchange: "", routingKey: "jobs" },
    chan1,
    {},
    enc("one"),
  );
  const first = await iterator.next();
  is(dec(first.value.body), "one", "a delivery arrives");
  // A second delivery arrives but is never read before the connection drops.
  await sendContent(
    c.brokers[0],
    "basic.deliver",
    { consumerTag: tag1, deliveryTag: 2n, redelivered: false, exchange: "", routingKey: "jobs" },
    chan1,
    {},
    enc("stale"),
  );
  await settle();

  // The socket goes.
  c.pilots[0].stop();
  c.brokers[0].end();
  await until(() => events.includes("up"), "the reconnect");
  ok(events[0].startsWith("down "), "the drop is reported");
  await until(() => c.pilots[1]?.names().includes("basic.consume"), "the re-consume");
  is(setups, 2, "setup runs again on the new connection");
  const names = c.pilots[1].names();
  ok(
    names.indexOf("confirm.select") < names.indexOf("queue.declare") &&
      names.indexOf("queue.declare") < names.indexOf("basic.qos") &&
      names.indexOf("basic.qos") < names.indexOf("basic.consume"),
    "in order: confirms, setup, prefetch, consume",
  );
  const qos = c.pilots[1].seen.find((m) => m.name === "basic.qos");
  is(qos.args.prefetchCount, 4, "the subscription's prefetch is re-applied");

  // The delivery from before the drop settles without sending anything.
  await first.value.ack();
  await settle();
  ok(!c.pilots[1].names().includes("basic.ack"), "a stale ack is not sent on the new channel");

  // The next delivery is the new channel's, not the stale buffered one.
  const consume2 = c.pilots[1].seen.find((m) => m.name === "basic.consume");
  await sendContent(
    c.brokers[1],
    "basic.deliver",
    {
      consumerTag: consume2.args.consumerTag,
      deliveryTag: 1n,
      redelivered: true,
      exchange: "",
      routingKey: "jobs",
    },
    consume2.channel,
    {},
    enc("again"),
  );
  const next = await iterator.next();
  is(dec(next.value.body), "again", "the unread delivery is not handed over; its redelivery is");
  await next.value.ack();
  await until(() => c.pilots[1].names().includes("basic.ack"), "the fresh ack");
  ok(true, "and a fresh delivery's ack is sent");

  // Publishing goes through the recovered channel, with confirms.
  await jobs.publish("", "jobs", "after");
  ok(c.pilots[1].names().includes("basic.publish"), "a publish uses the recovered channel");

  await subscription.cancel();
  is((await iterator.next()).done, true, "a cancelled subscription ends");
  await session.close();
}

{
  // A publish made while disconnected waits for the reconnect.
  const c = cluster();
  const session = await openSession("amqp://a/", base(c));
  const channel = await session.channel();
  await until(() => c.pilots[0] !== undefined, "the pilot");
  c.state.refuse = 2;
  c.pilots[0].stop();
  c.brokers[0].end();
  await until(() => !session.connected, "the drop");
  const publishing = channel.publish("", "q", "held");
  await until(() => c.pilots[1]?.names().includes("basic.publish"), "the held publish");
  await publishing;
  ok(true, "a publish made while disconnected goes out after the reconnect");

  // And one that waits past its timeout rejects.
  c.state.refuse = 1000;
  c.pilots[1].stop();
  c.brokers[1].end();
  await until(() => !session.connected, "the second drop");
  let failure = null;
  await channel.publish("", "q", "late", { timeout: 40 }).catch((e) => {
    failure = e;
  });
  is(failure?.code, "ERR_RABBITMQ_TIMEOUT", "a publish that outwaits its timeout rejects");
  const dials = c.state.dials;
  await session.close();
  await settle(60);
  is(c.state.dials, dials, "a closed session stops dialling");
}

{
  // A publish whose confirm was outstanding when the socket went is rejected,
  // not sent again.
  const c = cluster();
  const session = await openSession("amqp://a/", base(c));
  const channel = await session.channel({ confirm: true });
  await until(() => c.pilots[0] !== undefined, "the pilot");
  c.brokers[0].autoConfirm = false;
  let failure = null;
  const publishing = channel.publish("", "q", "unconfirmed").catch((e) => {
    failure = e;
  });
  await until(() => c.pilots[0].names().includes("basic.publish"), "the publish");
  c.pilots[0].stop();
  c.brokers[0].end();
  await publishing;
  ok(failure !== null, "an unconfirmed publish rejects when its connection is lost");
  await until(() => c.pilots[1]?.names().includes("confirm.select"), "the recovery");
  await settle(20);
  ok(!c.pilots[1].names().includes("basic.publish"), "and is not republished");
  await session.close();
}

{
  // A consumer the broker cancels — its queue deleted — ends the subscription.
  const c = cluster();
  const session = await openSession("amqp://a/", base(c));
  const channel = await session.channel();
  const subscription = channel.subscribe("q");
  const iterator = subscription[Symbol.asyncIterator]();
  await until(() => c.pilots[0]?.names().includes("basic.consume"), "the consume");
  const consume = c.pilots[0].seen.find((m) => m.name === "basic.consume");
  await c.brokers[0].send(
    "basic.cancel",
    { consumerTag: consume.args.consumerTag, noWait: true },
    consume.channel,
  );
  is((await iterator.next()).done, true, "a broker cancellation ends the subscription");
  await session.close();
}

{
  // A channel the broker closes is reopened, and set up again, on the same
  // connection.
  const c = cluster();
  const session = await openSession("amqp://a/", base(c));
  let setups = 0;
  const channel = await session.channel({
    setup: () => {
      setups++;
    },
  });
  const errors = [];
  session.onError = (e) => errors.push(e);
  const first = channel.current;
  await until(() => c.pilots[0] !== undefined, "the pilot");
  await c.brokers[0].send(
    "channel.close",
    { replyCode: 406, replyText: "PRECONDITION_FAILED", classId: 60, methodId: 80 },
    first.number,
  );
  await until(() => channel.current !== null && channel.current !== first, "the reopen");
  is(setups, 2, "setup runs again");
  is(errors[0]?.replyCode, 406, "the channel's failure is reported");
  is(c.state.dials, 1, "on the same connection");
  await session.close();
}

// The scripted brokers poll; stop them so the run ends on its own — and so a
// timer the session leaked after `close()` would keep it alive and show.
for (const stop of PILOTS) stop();
if (report("session") > 0) exit(1);
