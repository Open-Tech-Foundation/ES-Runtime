// Blocking commands, against a real server.
//
// The unit half (test/unit/blocking.mjs) settles which commands block and where
// each keeps its timeout. This settles what happens when one actually blocks.
import { exit, env } from "runtime:process";
import { DbErrorCode } from "runtime:db";

import { Redis, createPool } from "../dist/index.js";
import { is, ok, report } from "./unit/assert.mjs";

const url = env.REDIS_URL ?? "redis://127.0.0.1:6379";

const r = await Redis.connect(url);
await r.flushdb();

// -- the bounded forms ------------------------------------------------------

{
  const started = Date.now();
  is(await r.blpop("nothing", 1), null, "BLPOP that times out answers null");
  ok(Date.now() - started >= 900, "having waited its timeout");

  await r.rpush("q", "first", "second");
  is(await r.blpop("q", 1), { key: "q", value: "first" },
    "and answers { key, value } rather than a two-element array");
  is(await r.brpop("q", 1), { key: "q", value: "second" }, "BRPOP takes from the tail");

  // Several keys: the first with anything in it wins, which is why the reply
  // has to say which key it came from.
  await r.rpush("q2", "x");
  is(await r.blpop(["q1", "q2", "q3"], 1), { key: "q2", value: "x" },
    "with many keys, the reply names the one that had something");
}

{
  await r.del("src", "dst");
  await r.rpush("src", "moved");
  is(await r.blmove("src", "dst", 1), "moved", "BLMOVE returns the value it moved");
  is(await r.lrange("dst", 0, -1), ["moved"], "and it arrived");
  is(await r.blmove("src", "dst", 1), null, "an empty source times out");
}

{
  await r.del("z");
  await r.zadd("z", { low: 1, high: 9 });
  is(await r.bzpopmin("z", 1), { key: "z", member: "low", score: 1 },
    "BZPOPMIN answers the key, the member and a numeric score");
  is(await r.bzpopmax("z", 1), { key: "z", member: "high", score: 9 }, "BZPOPMAX the other end");
  is(await r.bzpopmin("z", 1), null, "an empty sorted set times out");
}

// WAIT against a server with no replicas returns 0 rather than blocking for its
// whole timeout — it is satisfied immediately because zero were asked for.
is(await r.wait(0, 100), 0, "WAIT with no replicas asked for answers 0");

// -- a blocking command really does block the connection --------------------

{
  // The reason the unbounded form is refused, demonstrated: everything else on
  // the connection waits behind it.
  const started = Date.now();
  const [popped, pinged] = await Promise.all([r.blpop("idle", 1), r.ping()]);
  const elapsed = Date.now() - started;
  is(popped, null, "the blocking command timed out");
  is(pinged, "PONG", "and the PING behind it eventually answered");
  ok(elapsed >= 900, `the PING waited for it (${elapsed}ms) — which is why 0 would never answer`);
}

// -- the unbounded form -----------------------------------------------------

{
  let code = null;
  try {
    await r.call(["BLPOP", "q", "0"]);
  } catch (e) {
    code = e.code;
  }
  is(code, DbErrorCode.Unsupported, "an unbounded BLPOP is refused on an ordinary connection");
  is(r.connection.blocking, false, "which is not a blocking connection");
}

{
  // Opted into: this connection exists to be tied up, so it is allowed.
  const worker = await Redis.connect(url, { blocking: true });
  is(worker.connection.blocking, true, "a { blocking: true } connection says so");

  // Prove it really blocks indefinitely and is released by data arriving.
  await r.del("handoff");
  const waiting = worker.call(["BLPOP", "handoff", "0"]);
  await new Promise((resolve) => setTimeout(resolve, 150));
  await r.rpush("handoff", "delivered");
  is(await waiting, ["handoff", "delivered"],
    "an unbounded BLPOP is allowed there, and unblocks when something arrives");
  await worker.close();
}

{
  // A pool's premise is that its connections come back, so an unbounded command
  // is refused through one whatever the option says.
  const pool = createPool(url, { blocking: true });
  let code = null;
  try {
    await pool.call(["BLPOP", "q", "0"]);
  } catch (e) {
    code = e.code;
  }
  is(code, DbErrorCode.Unsupported, "a pooled connection refuses the unbounded form");
  is(await pool.blpop("nothing", 1), null, "while the bounded form works through a pool");
  await pool.close();
}

// -- consume ----------------------------------------------------------------

{
  await r.del("jobs");
  await r.rpush("jobs", "a", "b", "c");
  const worker = await Redis.connect(url);
  const seen = [];
  for await (const job of worker.consume("jobs", { timeout: 1 })) {
    seen.push(job.value);
    if (seen.length === 3) break;
  }
  is(seen, ["a", "b", "c"], "consume yields queued jobs in order");
  // Breaking out left the connection usable, because the pop that delivered the
  // last job had already completed.
  is(await worker.ping(), "PONG", "and abandoning the loop leaves the connection usable");
  await worker.close();
}

{
  // An empty queue is not the end of the queue: consume keeps waiting, and a
  // job pushed later is delivered.
  await r.del("later");
  const worker = await Redis.connect(url);
  const seen = [];
  const running = (async () => {
    for await (const job of worker.consume("later", { timeout: 1 })) {
      seen.push(job.value);
      break;
    }
  })();
  await new Promise((resolve) => setTimeout(resolve, 150));
  await r.rpush("later", "eventually");
  await running;
  is(seen, ["eventually"], "consume waits through an empty queue and delivers a later job");
  await worker.close();
}

{
  // A signal stops the loop, which is what the bounded poll is for: an
  // unbounded wait could not notice.
  const worker = await Redis.connect(url);
  const controller = new AbortController();
  await r.del("never");
  const started = Date.now();
  const seen = [];
  const running = (async () => {
    for await (const job of worker.consume("never", { timeout: 1, signal: controller.signal })) {
      seen.push(job);
    }
  })();
  setTimeout(() => controller.abort(), 100);
  await running.catch(() => {});
  ok(Date.now() - started < 3000, "an aborted consume stops rather than looping forever");
  is(seen.length, 0, "having delivered nothing");
  await worker.close();
}

await r.flushdb();
await r.close();
if (report("blocking-live") > 0) exit(1);
