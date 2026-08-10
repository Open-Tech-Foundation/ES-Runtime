// Pub/sub, on a connection given over to it.
import { exit, env } from "runtime:process";
import { DbErrorCode } from "runtime:db";

import { Redis, createSubscriber } from "../dist/index.js";
import { is, ok, report } from "./unit/assert.mjs";

const url = env.REDIS_URL ?? "redis://127.0.0.1:6379";

/** Waits for `check` to hold, or gives up — messages arrive when they arrive. */
async function until(check, what, budget = 2000) {
  const deadline = Date.now() + budget;
  while (Date.now() < deadline) {
    if (check()) return true;
    await new Promise((r) => setTimeout(r, 10));
  }
  ok(false, `timed out waiting for ${what}`);
  return false;
}

const pub = await Redis.connect(url);

// -- a channel --------------------------------------------------------------

{
  const sub = await createSubscriber(url);
  const seen = [];
  await sub.subscribe("news", (payload, ctx) => seen.push([ctx.channel, payload]));

  is(sub.subscribed, true, "the connection is a subscriber now");
  is(sub.channels, ["news"], "and reports its channels");

  // Subscribing is confirmed, so publishing straight after cannot race it.
  is(await pub.publish("news", "hello"), 1, "PUBLISH reports one subscriber");
  await until(() => seen.length === 1, "the message");
  is(seen[0], ["news", "hello"], "which arrived with its channel");

  await pub.publish("news", "again");
  await until(() => seen.length === 2, "the second message");
  is(seen[1][1], "again", "and the next one");

  // A channel nobody subscribed to reaches nobody, and is not an error.
  is(await pub.publish("other", "ignored"), 0, "publishing to nobody answers 0");
  await new Promise((r) => setTimeout(r, 100));
  is(seen.length, 2, "and delivered nothing");

  await sub.close();
}

// -- several channels, and per-channel handlers -----------------------------

{
  const sub = await createSubscriber(url);
  const a = [];
  const b = [];
  await sub.subscribe(["alpha", "beta"], (p, c) => (c.channel === "alpha" ? a : b).push(p));
  is(sub.channels.sort(), ["alpha", "beta"], "subscribing to two at once");

  await pub.publish("alpha", "1");
  await pub.publish("beta", "2");
  await until(() => a.length === 1 && b.length === 1, "both messages");
  is([a[0], b[0]], ["1", "2"], "each went to its own channel");

  // A second handler on the same channel gets the message too.
  const also = [];
  await sub.subscribe("alpha", (p) => also.push(p));
  await pub.publish("alpha", "3");
  await until(() => also.length === 1, "the second handler");
  is(a.length, 2, "and the first handler still fires");

  await sub.close();
}

// -- the catch-all ----------------------------------------------------------

{
  const sub = await createSubscriber(url);
  const all = [];
  sub.onMessage = (p, c) => all.push(`${c.channel}:${p}`);
  await sub.subscribe("x");
  await pub.publish("x", "y");
  await until(() => all.length === 1, "the catch-all");
  is(all[0], "x:y", "onMessage sees messages with no per-channel handler");
  await sub.close();
}

// -- patterns ---------------------------------------------------------------

{
  const sub = await createSubscriber(url);
  const seen = [];
  await sub.psubscribe("news.*", (p, c) => seen.push([c.pattern, c.channel, p]));
  is(sub.patterns, ["news.*"], "the pattern is reported");

  await pub.publish("news.sport", "goal");
  await until(() => seen.length === 1, "the pattern message");
  is(seen[0], ["news.*", "news.sport", "goal"],
    "a pattern delivery carries the pattern and the concrete channel");

  await pub.publish("weather.today", "rain");
  await new Promise((r) => setTimeout(r, 100));
  is(seen.length, 1, "a channel outside the pattern is not delivered");
  await sub.close();
}

// -- unsubscribing ----------------------------------------------------------

{
  const sub = await createSubscriber(url);
  const seen = [];
  await sub.subscribe(["one", "two"], (p) => seen.push(p));

  await sub.unsubscribe("one");
  is(sub.channels, ["two"], "unsubscribing drops just that channel");
  await pub.publish("one", "gone");
  await pub.publish("two", "kept");
  await until(() => seen.length === 1, "the surviving channel's message");
  is(seen[0], "kept", "and the dropped one delivered nothing");

  await sub.unsubscribe();
  is(sub.channels, [], "unsubscribing with no argument drops them all");
  // The mode is not the subscription: the connection stays a subscriber.
  is(sub.subscribed, true, "the connection is still given over to subscribing");
  await sub.close();
}

// -- what a subscriber will not do ------------------------------------------

{
  const sub = await createSubscriber(url);
  await sub.subscribe("busy");

  let code = null;
  try {
    await sub.get("k");
  } catch (e) {
    code = e.code;
  }
  is(code, DbErrorCode.ConnectionBusy,
    "a subscribed connection refuses ordinary commands rather than hanging");

  // You cannot publish from the connection you subscribe on, which is the
  // reason a program that does both needs two.
  code = null;
  try {
    await sub.publish("busy", "x");
  } catch (e) {
    code = e.code;
  }
  is(code, DbErrorCode.ConnectionBusy, "including PUBLISH");
  await sub.close();
}

{
  // A raw SUBSCRIBE would bypass the read loop's bookkeeping, so it is refused
  // by name — pointing at the method that does it properly.
  const r = await Redis.connect(url);
  let message = null;
  try {
    await r.call(["SUBSCRIBE", "raw"]);
  } catch (e) {
    message = e.message;
  }
  ok(message !== null && message.includes("subscribe"), "a raw SUBSCRIBE points at the API");
  is(await r.ping(), "PONG", "and the connection is untouched by the refusal");
  await r.close();
}

// -- a handler that throws --------------------------------------------------

{
  // The read loop is the only thing reading this socket. One bad handler must
  // not end it, or every other subscription on the connection stops silently.
  const sub = await createSubscriber(url);
  const errors = [];
  const good = [];
  sub.onSubscribeError = (e) => errors.push(e);
  await sub.subscribe("boom", () => {
    throw new Error("handler blew up");
  });
  await sub.subscribe("fine", (p) => good.push(p));

  await pub.publish("boom", "1");
  await until(() => errors.length === 1, "the handler's error");
  is(errors[0].message, "handler blew up", "which is reported rather than swallowed");

  await pub.publish("fine", "2");
  await until(() => good.length === 1, "a later message on another channel");
  is(good[0], "2", "so the loop survived the bad handler");
  await sub.close();
}

// -- binary payloads --------------------------------------------------------

{
  const sub = await createSubscriber(url, { binary: true });
  const seen = [];
  await sub.subscribe("bytes", (p) => seen.push(p));
  await pub.publish("bytes", new Uint8Array([0xff, 0x00, 0xfe]));
  await until(() => seen.length === 1, "the binary message");
  ok(seen[0] instanceof Uint8Array, "binary mode reaches the subscriber");
  is([...seen[0]], [255, 0, 254], "with bytes that survived exactly");
  await sub.close();
}

// -- introspection ----------------------------------------------------------

{
  const sub = await createSubscriber(url);
  await sub.subscribe(["room:1", "room:2"]);
  const channels = await pub.pubsubChannels("room:*");
  is(channels.sort(), ["room:1", "room:2"], "PUBSUB CHANNELS sees them from another connection");
  is(await pub.pubsubNumsub("room:1"), { "room:1": 1 }, "and PUBSUB NUMSUB counts them");
  await sub.close();
}

// -- RESP2 ------------------------------------------------------------------

{
  // A genuinely different branch, not a variation: RESP3 delivers these as
  // push frames and RESP2 as ordinary arrays, so the reader tells a message
  // from a reply by its *content* there rather than by its type byte.
  const sub = await createSubscriber(`${url}?protocol=2`);
  is(sub.protocol, 2, "the subscriber is on RESP2");
  const seen = [];
  await sub.subscribe("legacy", (p, c) => seen.push([c.channel, p]));
  await pub.publish("legacy", "hello");
  await until(() => seen.length === 1, "a RESP2 message");
  is(seen[0], ["legacy", "hello"], "which arrives the same shape as on RESP3");

  const patterned = [];
  await sub.psubscribe("leg.*", (p, c) => patterned.push([c.pattern, c.channel, p]));
  await pub.publish("leg.acy", "pat");
  await until(() => patterned.length === 1, "a RESP2 pattern message");
  is(patterned[0], ["leg.*", "leg.acy", "pat"], "and so does a pattern delivery");

  await sub.unsubscribe("legacy");
  is(sub.channels, [], "unsubscribing is confirmed over RESP2 too");
  await sub.close();
}

// -- closing a subscriber ---------------------------------------------------

{
  const sub = await createSubscriber(url);
  await sub.subscribe("closing");
  const errors = [];
  sub.onSubscribeError = (e) => errors.push(e);
  await sub.close();
  await new Promise((r) => setTimeout(r, 100));
  is(errors.length, 0, "closing a subscriber is not an error the read loop reports");
}

await pub.close();
if (report("pubsub") > 0) exit(1);
