// Reopening a connection the server took away — and what must not come back.
import { exit, env } from "runtime:process";
import { DbErrorCode } from "runtime:db";

import { listen } from "runtime:net";

import { Redis, createSubscriber } from "../dist/index.js";
import { is, ok, report } from "./unit/assert.mjs";

const url = env.REDIS_URL ?? "redis://127.0.0.1:6379";

/** Kills a server-side connection by id, from another one. */
async function killId(id) {
  const executioner = await Redis.connect(url);
  await executioner.call(["CLIENT", "KILL", "ID", String(id)]);
  await executioner.close();
  // Let the close reach us.
  await new Promise((resolve) => setTimeout(resolve, 100));
}

/** Kills `victim`'s connection. Takes its id first, while it is still idle. */
async function kill(victim) {
  await killId(await victim.call(["CLIENT", "ID"]));
}

async function until(check, what, budget = 4000) {
  const deadline = Date.now() + budget;
  while (Date.now() < deadline) {
    if (check()) return true;
    await new Promise((r) => setTimeout(r, 20));
  }
  ok(false, `timed out waiting for ${what}`);
  return false;
}

// -- off by default ---------------------------------------------------------

{
  const r = await Redis.connect(url);
  is(r.connection.reconnects, false, "reconnection is off unless asked for");
  await kill(r);
  let code = null;
  try {
    await r.ping();
  } catch (e) {
    code = e.code;
  }
  is(code, DbErrorCode.ConnectionLost, "so a killed connection stays dead");
  await r.close();
}

// -- reopening --------------------------------------------------------------

{
  const r = await Redis.connect(url, { reconnect: true });
  is(r.connection.reconnects, true, "{ reconnect: true } turns it on");
  await r.set("survives", "yes");
  await kill(r);
  is(await r.ping(), "PONG", "the next command reopens the connection");
  is(await r.get("survives"), "yes", "and it is the same server");
  ok(r.usable, "the connection reports itself usable again");
  await r.close();
}

{
  // The database and the client name are connection configuration, so they are
  // restored — a reopened connection pointing at db 0 would silently move every
  // later key.
  const r = await Redis.connect(`${url}/7?client_name=reconnect-test`, { reconnect: true });
  await r.call(["FLUSHDB"]);
  await r.set("in-seven", "1");
  await kill(r);
  is(await r.get("in-seven"), "1", "the selected database is restored");
  const name = await r.call(["CLIENT", "GETNAME"]);
  is(name, "reconnect-test", "and the client name");
  await r.call(["FLUSHDB"]);
  await r.close();
}

{
  // A burst arriving after the server went away must reopen once, not once per
  // caller.
  const r = await Redis.connect(url, { reconnect: true });
  await kill(r);
  const answers = await Promise.all(Array.from({ length: 10 }, () => r.ping()));
  is(answers.length, 10, "ten concurrent commands all completed");
  ok(answers.every((a) => a === "PONG"), "and every one of them answered");
  await r.close();
}

// -- what is deliberately not replayed --------------------------------------

{
  // The command that was in flight is not retried. Whether the server ran it
  // before the socket died is not knowable, and replaying INCR would
  // double-count — so its caller sees the failure.
  const r = await Redis.connect(url, { reconnect: true });
  await r.set("counter", "0");
  // The id is taken *before* the blocking command starts: asking afterwards
  // would queue behind it and the kill would land after it had finished.
  const id = await r.call(["CLIENT", "ID"]);
  // The outcome is captured at once rather than awaited two turns later: a
  // rejected promise nobody has attached a handler to by the end of the
  // microtask checkpoint is reported as unhandled, which would be this test
  // creating noise rather than the driver.
  const inflight = r.call(["BLPOP", "never-arrives", "5"]).then(
    (value) => ({ ok: true, value }),
    (error) => ({ ok: false, error }),
  );
  await new Promise((resolve) => setTimeout(resolve, 100));
  await killId(id);
  const outcome = await inflight;
  ok(!outcome.ok, "the command that was in flight fails rather than being retried");
  is(outcome.error?.code, DbErrorCode.ConnectionLost, "as a lost connection");
  is(await r.ping(), "PONG", "while the connection itself is back");
  await r.close();
}

{
  // A WATCH is void after a reconnect: the server has no memory of it, so an
  // EXEC that went ahead would report a guarantee nobody is making.
  const r = await Redis.connect(url, { reconnect: true });
  await r.set("guarded", "1");
  await r.watch("guarded");
  await kill(r);

  const tx = r.multi();
  tx.set("guarded", "2");
  let code = null;
  try {
    await tx.exec();
  } catch (e) {
    code = e.code;
  }
  is(code, DbErrorCode.SerializationFailure,
    "an EXEC after a reconnect that dropped a WATCH fails rather than succeeding");
  is(await r.get("guarded"), "1", "and nothing was written");

  // Once acknowledged, the connection is ordinary again.
  const after = r.multi();
  after.set("guarded", "3");
  is(await after.exec(), ["OK"], "the next transaction works normally");
  is(await r.get("guarded"), "3", "and applied");
  await r.close();
}

{
  // An open MULTI went with the connection, so the pool's cleanliness check
  // must not still believe one is open.
  const r = await Redis.connect(url, { reconnect: true });
  // The id first: inside a MULTI every command answers QUEUED, including the
  // one that would tell us which connection to kill.
  const id = await r.call(["CLIENT", "ID"]);
  await r.call(["MULTI"]);
  is(r.connection.clean, false, "a connection inside MULTI is not clean");
  await killId(id);
  await r.ping();
  is(r.connection.clean, true, "and after a reconnect the MULTI is forgotten");
  await r.close();
}

// -- giving up --------------------------------------------------------------

{
  // A server that goes away for good. Rather than stopping a container, this
  // stands up a socket that speaks just enough RESP to be connected to and then
  // stops listening — so reopening genuinely cannot succeed, and the bound on
  // attempts is what ends it.
  const listener = listen({ hostname: "127.0.0.1", port: 0 });
  const { port } = await listener.addr;

  const served = (async () => {
    const socket = await listener.accept();
    if (socket === null) return;
    const writer = socket.writable.getWriter();
    // Refusing HELLO is how a pre-6 server answers, and it puts the client on
    // RESP2 with no password to send — so the handshake is those two frames and
    // nothing else, which is the shortest legal way to be a Redis.
    const reader = socket.readable.getReader();
    await reader.read();
    await writer.write(new TextEncoder().encode("-ERR unknown command 'HELLO'\r\n"));
    // Then answer one PING and hang up for good.
    await reader.read();
    await writer.write(new TextEncoder().encode("+PONG\r\n"));
    await socket.close().catch(() => {});
  })();

  const doomed = await Redis.connect(`redis://127.0.0.1:${port}`, {
    reconnect: { attempts: 2, delay: 20, maxDelay: 40 },
  });
  is(await doomed.ping(), "PONG", "the stand-in server answered once");

  await served;
  await listener.close();

  const started = Date.now();
  let code = null;
  try {
    await doomed.ping();
  } catch (e) {
    code = e.code;
  }
  is(code, DbErrorCode.ConnectionLost, "giving up reports the connection as lost");
  ok(Date.now() - started < 4000, "after a bounded number of attempts rather than forever");
  await doomed.close();
}

// -- subscribers reconnect themselves ---------------------------------------

{
  // Nobody calls a subscriber, so the lazy path every other command takes would
  // never run. Its read loop reopens instead, and puts the subscriptions back.
  const sub = await createSubscriber(url, { reconnect: { delay: 20 } });
  // Before subscribing: a subscribed connection runs no ordinary commands, so
  // asking it for its own id afterwards is refused.
  const subId = await sub.call(["CLIENT", "ID"]);
  const seen = [];
  await sub.subscribe("resilient", (payload) => seen.push(payload));
  await sub.psubscribe("res.*", (payload) => seen.push(`p:${payload}`));

  const pub = await Redis.connect(url);
  await pub.publish("resilient", "before");
  await until(() => seen.length === 1, "the first message");

  await killId(subId);
  await until(() => sub.connection.usable, "the subscriber to reopen");
  is(sub.channels, ["resilient"], "it still knows its channels");
  is(sub.patterns, ["res.*"], "and its patterns");

  // The server has to actually have the subscription again, which only a
  // delivered message proves.
  // Only the server saying so proves the subscription is really back — the
  // client's own bookkeeping would say yes either way.
  const deadline = Date.now() + 4000;
  let restored = false;
  while (Date.now() < deadline && !restored) {
    restored = (await pub.pubsubNumsub("resilient"))["resilient"] === 1;
    if (!restored) await new Promise((resolve) => setTimeout(resolve, 20));
  }
  ok(restored, "the server sees the subscription again");
  await pub.publish("resilient", "after");
  await until(() => seen.includes("after"), "a message after the reconnect");
  ok(seen.includes("after"), "messages are delivered again");

  await pub.publish("res.tored", "pattern");
  await until(() => seen.includes("p:pattern"), "a pattern message after the reconnect");
  ok(seen.includes("p:pattern"), "and pattern subscriptions came back too");

  await pub.close();
  await sub.close();
}

if (report("reconnect") > 0) exit(1);
