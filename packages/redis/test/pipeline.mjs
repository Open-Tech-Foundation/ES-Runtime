// Pipelining: many commands, one round trip, and no atomicity implied.
import { exit, env } from "runtime:process";
import { DbError, queryAst, connect } from "runtime:db";

import { Redis, createPool } from "../dist/index.js";
import { is, ok, report } from "./unit/assert.mjs";

const url = env.REDIS_URL ?? "redis://127.0.0.1:6379";
const r = await Redis.connect(url);
await r.flushdb();

// -- the basics -------------------------------------------------------------

{
  const p = r.pipeline();
  p.set("a", "1");
  p.set("b", "2");
  const a = p.get("a");
  p.incr("n");
  is(p.size, 4, "commands are queued rather than sent");

  const results = await p.exec();
  is(results, ["OK", "OK", "1", 1], "one result per command, in order");
  is(await a, "1", "and each queued call is a promise for its own result");
  is(await r.get("b"), "2", "the commands applied");
}

is(await r.pipeline().exec(), [], "an empty pipeline is an empty result");

// -- it is not a transaction ------------------------------------------------

{
  // A failure does not stop the rest — the whole batch was already on the wire.
  await r.set("str", "not-a-list");
  const p = r.pipeline();
  p.set("before", "1");
  p.call(["LPUSH", "str", "boom"]);
  p.set("after", "1");
  const results = await p.exec();

  ok(results[1] instanceof DbError, "a failed command's result is a DbError in place");
  is(results[1].backendCode, "WRONGTYPE", "carrying Redis's own word");
  is(await r.get("before"), "1", "the one before it applied");
  is(await r.get("after"), "1", "and so did the one after");
}

{
  // Unlike a transaction, a command refused at parse time does not discard the
  // batch: there is no MULTI to abort.
  await r.del("survivor");
  const p = r.pipeline();
  p.call(["NOSUCHCOMMAND"]);
  p.set("survivor", "1");
  const results = await p.exec();
  ok(results[0] instanceof DbError, "the bad command failed");
  is(await r.get("survivor"), "1", "and the good one still ran");
}

// -- it really is one round trip --------------------------------------------

{
  // 500 commands one at a time against 500 pipelined. This is a latency
  // comparison, not a benchmark: the point is the shape, and even on a loopback
  // socket where a round trip is nearly free the difference has to show.
  const N = 500;
  await r.del("counter");

  const serialStart = Date.now();
  for (let i = 0; i < N; i++) await r.incr("counter");
  const serial = Date.now() - serialStart;

  await r.del("counter");
  const pipedStart = Date.now();
  const p = r.pipeline();
  for (let i = 0; i < N; i++) p.incr("counter");
  const results = await p.exec();
  const piped = Date.now() - pipedStart;

  is(results.length, N, `${N} results came back`);
  is(results[N - 1], N, "counting up to the last one");
  is(Number(await r.get("counter")), N, "and the server agrees");
  ok(piped < serial, `pipelined ${piped}ms beat serial ${serial}ms`);
  console.log(`    ${N} commands: ${serial}ms serial, ${piped}ms pipelined`);
}

// -- ordering under concurrency ---------------------------------------------

{
  // Two pipelines in flight at once must not interleave their replies: the
  // connection lock is the only thing making reply i belong to command i.
  await r.flushdb();
  const [first, second] = await Promise.all([
    (() => {
      const p = r.pipeline();
      for (let i = 0; i < 50; i++) p.set(`x${i}`, `x${i}`);
      for (let i = 0; i < 50; i++) p.get(`x${i}`);
      return p.exec();
    })(),
    (() => {
      const p = r.pipeline();
      for (let i = 0; i < 50; i++) p.set(`y${i}`, `y${i}`);
      for (let i = 0; i < 50; i++) p.get(`y${i}`);
      return p.exec();
    })(),
  ]);
  is(first.slice(50).join(","), Array.from({ length: 50 }, (_, i) => `x${i}`).join(","),
    "the first pipeline's replies are all its own");
  is(second.slice(50).join(","), Array.from({ length: 50 }, (_, i) => `y${i}`).join(","),
    "and so are the second's");
}

// -- misuse -----------------------------------------------------------------

{
  const p = r.pipeline();
  p.set("k", "v");
  await p.exec();
  let threw = false;
  try {
    await p.exec();
  } catch {
    threw = true;
  }
  ok(threw, "a pipeline cannot be executed twice");

  const dropped = r.pipeline();
  dropped.set("nope", "1");
  dropped.discard();
  is(await r.get("nope"), null, "discard sends nothing");
}

// -- executeMany, which now pipelines ---------------------------------------

{
  const db = await connect(url);
  await db.execute(queryAst(["FLUSHDB"]));

  const result = await db.executeMany(queryAst(["SET"]), [
    ["k1", "1"],
    ["k2", "2"],
    ["k3", "3"],
  ]);
  is(result.changes, 3, "executeMany reports every set");
  is((await (await db.query(queryAst(["MGET", "k1", "k2", "k3"]))).toArray()).map((x) => x.value),
    ["1", "2", "3"], "and all of them applied");

  // Every set is attempted, where the old loop stopped at the first failure —
  // a pipeline has already sent them all.
  await db.execute(queryAst(["SET", "wrong", "string"]));
  let failed = null;
  try {
    await db.executeMany(queryAst(["LPUSH"]), [["ok-list", "a"], ["wrong", "b"], ["ok-list", "c"]]);
  } catch (e) {
    failed = e;
  }
  ok(failed !== null, "a failing set fails the call");
  is(failed.backendCode, "WRONGTYPE", "with the reason");
  is((await (await db.query(queryAst(["LRANGE", "ok-list", 0, -1]))).toArray()).map((x) => x.value).sort(),
    ["a", "c"], "and the sets around it still ran — there is no transaction here");

  await db.close();
}

// -- through a pool ---------------------------------------------------------

{
  const pool = createPool(url);
  const p = pool.pipeline();
  p.set("pooled", "1");
  p.get("pooled");
  is(await p.exec(), ["OK", "1"], "a pool runs a pipeline on one connection");
  is(pool.idle, 1, "and gives it straight back");
  await pool.close();
}

await r.flushdb();
await r.close();
if (report("pipeline") > 0) exit(1);
