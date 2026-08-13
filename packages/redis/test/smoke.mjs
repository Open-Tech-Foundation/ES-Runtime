// The client surface, against a real server.

import { connect } from "runtime:db";
import { env, exit } from "runtime:process";

import { driver as redis } from "../dist/index.js";
import { is, ok, report } from "./unit/assert.mjs";

const url = env.REDIS_URL ?? "redis://127.0.0.1:6379";
const r = await connect(url, { driver: redis });
await r.flushdb();

is(r.protocol, 3, "RESP3 is negotiated against a modern server");
ok(typeof r.hello.version === "string", `the server identified itself as ${r.hello.version}`);

// -- strings ----------------------------------------------------------------

is(await r.set("k", "v"), "OK", "SET answers a status");
is(await r.get("k"), "v", "GET reads it back");
is(await r.get("absent"), null, "a missing key is null, not an empty string");
is(await r.setnx("k", "other"), false, "SETNX declines an existing key");
is(await r.get("k"), "v", "and did not overwrite it");
is(await r.set("k", "new", { xx: true }), "OK", "XX applies to an existing key");
is(
  await r.set("absent", "x", { nx: false, xx: true }),
  null,
  "XX on a missing key answers null rather than throwing",
);

is(await r.set("old", "1", { get: true }), null, "SET GET on a missing key answers null");
is(await r.set("old", "2", { get: true }), "1", "and the previous value once there is one");

is(await r.append("k", "!"), 4, "APPEND answers the new length");
is(await r.strlen("k"), 4, "STRLEN agrees");
is(await r.mset({ a: "1", b: "2" }), "OK", "MSET takes an object");
is(await r.mget("a", "b", "absent"), ["1", "2", null], "MGET keeps the holes");

// -- counters ---------------------------------------------------------------

is(await r.incr("n"), 1, "INCR answers the new value");
is(await r.incrBy("n", 41), 42, "INCRBY too");
is(await r.decr("n"), 41, "DECR");
ok(
  (await r.incrBy("huge", 9007199254740993n)) === 9007199254740993n,
  "a counter past 2^53 stays exact as a bigint",
);
is(await r.incrByFloat("f", 1.5), 1.5, "INCRBYFLOAT answers a number");

// -- expiry -----------------------------------------------------------------

await r.set("t", "1", { ex: 100 });
ok((await r.ttl("t")) > 90, "SET EX sets a TTL");
is(await r.persist("t"), true, "PERSIST clears it");
is(await r.ttl("t"), -1, "-1 means no expiry");
is(await r.ttl("absent"), -2, "-2 means no key — distinct from no expiry");
is(await r.expire("t", 50), true, "EXPIRE applies");
is(await r.expire("absent", 50), false, "and declines a key that is not there");

// -- keys -------------------------------------------------------------------

is(await r.type("k"), "string", "TYPE");
is(await r.type("absent"), "none", "TYPE of a missing key");
is(await r.exists("k", "a", "absent"), 2, "EXISTS counts the ones present");
is(await r.del("old"), 1, "DEL answers how many went");
is(await r.del("old"), 0, "and zero the second time");

// -- hashes -----------------------------------------------------------------

is(await r.hset("h", { one: "1", two: "2" }), 2, "HSET with an object answers fields added");
is(await r.hset("h", "three", "3"), 1, "HSET with a field and a value");
is(await r.hset("h", "three", "3"), 0, "overwriting adds nothing");
is(await r.hget("h", "one"), "1", "HGET");
is(await r.hget("h", "absent"), null, "a missing field is null");
is(await r.hgetall("h"), { one: "1", two: "2", three: "3" }, "HGETALL is an object");
is(await r.hgetall("absent"), {}, "and an empty one for a missing key");
is(await r.hlen("h"), 3, "HLEN");
is(await r.hexists("h", "one"), true, "HEXISTS");
is((await r.hkeys("h")).sort(), ["one", "three", "two"], "HKEYS");
is(await r.hincrBy("h", "one", 10), 11, "HINCRBY");
is(await r.hdel("h", "one", "two"), 2, "HDEL");

// -- lists ------------------------------------------------------------------

is(await r.rpush("list", "a", "b", "c"), 3, "RPUSH answers the new length");
is(await r.lrange("list", 0, -1), ["a", "b", "c"], "LRANGE, with an inclusive stop");
is(await r.lrange("list", 0, 0), ["a"], "and a narrower range");
is(await r.llen("list"), 3, "LLEN");
is(await r.lpop("list"), "a", "LPOP takes from the head");
is(await r.rpop("list"), "c", "RPOP from the tail");
is(await r.lpop("absent"), null, "popping nothing is null");
is(await r.lrange("absent", 0, -1), [], "a missing list ranges to an empty array");

// -- sets -------------------------------------------------------------------

is(await r.sadd("s", "x", "y", "x"), 2, "SADD counts what was actually added");
is((await r.smembers("s")).sort(), ["x", "y"], "SMEMBERS");
is(await r.sismember("s", "x"), true, "SISMEMBER");
is(await r.sismember("s", "z"), false, "and says no");
is(await r.scard("s"), 2, "SCARD");
is(await r.srem("s", "x"), 1, "SREM");

// -- sorted sets ------------------------------------------------------------

is(await r.zadd("z", { one: 1, two: 2, three: 3 }), 3, "ZADD takes { member: score }");
is(await r.zrange("z", 0, -1), ["one", "two", "three"], "ZRANGE is in score order");
is(
  await r.zrange("z", 0, -1, { withScores: true }),
  [
    ["one", 1],
    ["two", 2],
    ["three", 3],
  ],
  "WITHSCORES pairs them, whatever the protocol interleaved",
);
is(await r.zscore("z", "two"), 2, "ZSCORE is a number");
is(await r.zscore("z", "absent"), null, "and null for a member that is not there");
is(await r.zcard("z"), 3, "ZCARD");
is(await r.zrank("z", "one"), 0, "ZRANK is zero-based");
is(await r.zrank("z", "absent"), null, "and null when there is no rank");
is(await r.zrange("z", 0, -1, { rev: true }), ["three", "two", "one"], "REV reverses it");

// -- scanning ---------------------------------------------------------------

{
  // The property that matters: walking to the end sees everything, whatever the
  // pages looked like on the way.
  const seen = new Set();
  for await (const key of r.scanIterator({ count: 3 })) seen.add(key);
  const keys = new Set(await r.keys("*"));
  is(seen.size, keys.size, `SCAN saw every key KEYS did (${keys.size})`);
  ok(
    [...keys].every((key) => seen.has(key)),
    "and the same ones",
  );
}

// -- scripting --------------------------------------------------------------

is(await r.eval("return 1", [], []), 1, "EVAL returns an integer");
is(await r.eval("return redis.call('GET', KEYS[1])", ["k"], []), "new!", "a script reads a key");
{
  const sha = await r.scriptLoad("return 'cached'");
  is(await r.evalsha(sha, [], []), "cached", "SCRIPT LOAD then EVALSHA");
}

// -- server -----------------------------------------------------------------

is(await r.ping(), "PONG", "PING");
is(await r.echo("hello"), "hello", "ECHO");
ok((await r.dbsize()) > 0, "DBSIZE");
ok((await r.info("server")).includes("redis_version"), "INFO returns the raw text");
ok(Array.isArray(await r.time()), "TIME is a pair");

// -- the escape hatch -------------------------------------------------------

// Anything without a helper is still one call away, which is what makes the
// helper list a convenience rather than a ceiling.
is(await r.call(["OBJECT", "ENCODING", "list"]), "listpack", "an unwrapped command");

// -- blocking commands ------------------------------------------------------

{
  // Bounded is allowed: the connection is held for the timeout, and that is a
  // cost the caller chose knowingly.
  const started = Date.now();
  is(
    await r.call(["BLPOP", "empty-queue", "1"]),
    null,
    "a bounded BLPOP times out and answers null",
  );
  ok(Date.now() - started >= 900, "having actually waited");

  // And it does return a value when there is one.
  await r.rpush("queue", "job");
  is(
    await r.call(["BLPOP", "queue", "1"]),
    ["queue", "job"],
    "and pops when the list has something",
  );

  // Unbounded is refused, because it would never give the connection back.
  let refused = null;
  try {
    await r.call(["BLPOP", "empty-queue", "0"]);
  } catch (e) {
    refused = e.message;
  }
  ok(refused?.includes("BLPOP"), "an unbounded BLPOP is refused by name");
  is(await r.ping(), "PONG", "and the connection still works afterwards");
}

await r.flushdb();
await r.close();
if (report("smoke") > 0) exit(1);
