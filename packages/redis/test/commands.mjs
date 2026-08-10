// The command families added beyond the core: streams, geo, bitmaps,
// HyperLogLog, hash-field TTLs, and the odds and ends.
import { exit, env } from "runtime:process";

import { connect } from "runtime:db";

import redis from "../dist/index.js";
import { is, ok, report } from "./unit/assert.mjs";

const url = env.REDIS_URL ?? "redis://127.0.0.1:6379";
const r = await connect(url, { driver: redis });
await r.flushdb();

const version = Number((await r.info("server")).match(/redis_version:(\d+)/)?.[1] ?? 0);

// -- string ranges and bits -------------------------------------------------

{
  await r.set("s", "Hello World");
  is(await r.getrange("s", 0, 4), "Hello", "GETRANGE, with an inclusive end");
  is(await r.getrange("s", -5, -1), "World", "and negative offsets");
  is(await r.setrange("s", 6, "Redis"), 11, "SETRANGE answers the new length");
  is(await r.get("s"), "Hello Redis", "and overwrote in place");

  await r.del("b");
  is(await r.setbit("b", 7, 1), 0, "SETBIT answers the bit that was there");
  is(await r.getbit("b", 7), 1, "GETBIT reads it back");
  is(await r.get("b"), "\x01", "one bit at offset 7 is the byte 0x01");
  is(await r.bitcount("b"), 1, "BITCOUNT counts set bits");
  is(await r.bitpos("b", 1), 7, "BITPOS finds the first set bit");

  // Written as bytes, not as a string: "\xff" is U+00FF, which is *two* bytes
  // in UTF-8 — and a bitmap that went through a text encoding is not the
  // bitmap anyone meant.
  await r.set("x", new Uint8Array([0xff]));
  await r.set("y", new Uint8Array([0x0f]));
  is(await r.strlen("x"), 1, "a byte written as bytes is one byte long");
  await r.bitop("AND", "z", "x", "y");
  is(await r.bitcount("z"), 4, "BITOP AND wrote the intersection (0xff & 0x0f = 0x0f)");
}

// -- HyperLogLog ------------------------------------------------------------

{
  await r.del("hll", "hll2", "merged");
  is(await r.pfadd("hll", "a", "b", "c"), true, "PFADD changed the estimate");
  is(await r.pfcount("hll"), 3, "PFCOUNT is exact at small cardinalities");
  await r.pfadd("hll2", "c", "d");
  is(await r.pfcount("hll", "hll2"), 4, "and counts a union across keys");
  await r.pfmerge("merged", "hll", "hll2");
  is(await r.pfcount("merged"), 4, "PFMERGE combines them");

  // The property it exists for: many members, constant space.
  await r.del("big");
  const p = r.pipeline();
  for (let i = 0; i < 5000; i++) p.pfadd("big", `member-${i}`);
  await p.exec();
  const estimate = await r.pfcount("big");
  ok(Math.abs(estimate - 5000) < 5000 * 0.05, `5000 members estimated as ${estimate}`);
  ok((await r.strlen("big")) < 13000, "in about 12 KB whatever the cardinality");
}

// -- streams ----------------------------------------------------------------

{
  await r.del("events");
  const first = await r.xadd("events", { kind: "created", id: "1" });
  ok(/^\d+-\d+$/.test(first), `XADD answered a generated id (${first})`);
  await r.xadd("events", { kind: "updated", id: "1" });
  const third = await r.xadd("events", { kind: "deleted", id: "1" });
  is(await r.xlen("events"), 3, "XLEN");

  const all = await r.xrange("events");
  is(all.length, 3, "XRANGE reads them all");
  is(all[0].fields, { kind: "created", id: "1" }, "with fields as an object");
  is(all[0].id, first, "and the id it was given");

  const reversed = await r.xrevrange("events");
  is(reversed[0].id, third, "XREVRANGE starts at the other end");
  is((await r.xrange("events", "-", "+", { count: 2 })).length, 2, "COUNT limits it");

  is(await r.xdel("events", third), 1, "XDEL removes one");
  is(await r.xlen("events"), 2, "and the stream is shorter");
}

{
  // XREAD from the beginning, which is the non-blocking shape.
  await r.del("s1", "s2");
  await r.xadd("s1", { n: "1" });
  await r.xadd("s2", { n: "2" });
  const read = await r.xread({ s1: "0", s2: "0" });
  is(Object.keys(read).sort(), ["s1", "s2"], "XREAD keys its result by stream");
  is(read.s1[0].fields, { n: "1" }, "with each stream's entries");

  // A bounded BLOCK is allowed; nothing new means an empty result, not an error.
  const nothing = await r.xread({ s1: "$" }, { block: 100 });
  is(Object.keys(nothing).length, 0, "a blocking XREAD that times out reads no streams");

  // And an unbounded one is refused, like every other blocking command.
  let refused = false;
  try {
    await r.xread({ s1: "$" }, { block: 0 });
  } catch {
    refused = true;
  }
  ok(refused, "XREAD BLOCK 0 is refused");
}

{
  // Consumer groups.
  await r.del("jobs");
  await r.xgroupCreate("jobs", "workers", "0", { mkstream: true });
  await r.xadd("jobs", { task: "a" });
  await r.xadd("jobs", { task: "b" });

  const taken = await r.xreadgroup("workers", "worker-1", { jobs: ">" });
  is(taken.jobs.length, 2, "a consumer takes the pending entries");
  is(taken.jobs[0].fields.task, "a", "in order");

  // A second consumer sees nothing new, because the first has them.
  const second = await r.xreadgroup("workers", "worker-2", { jobs: ">" });
  is(Object.keys(second).length, 0, "and another consumer sees none of them");

  is(await r.xack("jobs", "workers", taken.jobs[0].id), 1, "XACK removes one from pending");
  is(await r.xgroupDestroy("jobs", "workers"), true, "XGROUP DESTROY");
}

{
  await r.del("capped");
  for (let i = 0; i < 100; i++) await r.xadd("capped", { i: String(i) }, { maxlen: 10, approximate: false });
  is(await r.xlen("capped"), 10, "MAXLEN keeps a stream bounded");
  is(await r.xtrim("capped", { maxlen: 5, approximate: false }), 5, "XTRIM removes the excess");
  is(await r.xlen("capped"), 5, "leaving the newest");
}

// -- geo --------------------------------------------------------------------

{
  await r.del("cities");
  is(await r.geoadd("cities", {
    Palermo: [13.361389, 38.115556],
    Catania: [15.087269, 37.502669],
  }), 2, "GEOADD adds points");

  const distance = await r.geodist("cities", "Palermo", "Catania", "km");
  ok(Math.abs(distance - 166.27) < 1, `GEODIST in km (${distance})`);
  is(await r.geodist("cities", "Palermo", "Nowhere"), null, "and null for a missing member");

  const [palermo, missing] = await r.geopos("cities", "Palermo", "Nowhere");
  ok(Math.abs(palermo.longitude - 13.361389) < 0.001, "GEOPOS returns the longitude");
  ok(Math.abs(palermo.latitude - 38.115556) < 0.001, "and the latitude");
  is(missing, null, "with null for a member that is not there");

  const near = await r.geosearch("cities", { fromMember: "Palermo", byRadius: 200, unit: "km" });
  is(near.sort(), ["Catania", "Palermo"], "GEOSEARCH finds both within 200km");
  const closer = await r.geosearch("cities", { fromMember: "Palermo", byRadius: 100, unit: "km" });
  is(closer, ["Palermo"], "and only one within 100km");
}

// -- scan iterators ---------------------------------------------------------

{
  await r.del("bighash", "bigset", "bigzset");
  const seed = r.pipeline();
  for (let i = 0; i < 300; i++) {
    seed.hset("bighash", `f${i}`, String(i));
    seed.sadd("bigset", `m${i}`);
    seed.zadd("bigzset", { [`z${i}`]: i });
  }
  await seed.exec();

  const fields = {};
  for await (const [field, value] of r.hscanIterator("bighash", { count: 10 })) fields[field] = value;
  is(Object.keys(fields).length, 300, "hscanIterator walks every field");
  is(fields.f299, "299", "with the right values");

  const members = new Set();
  for await (const member of r.sscanIterator("bigset", { count: 10 })) members.add(member);
  is(members.size, 300, "sscanIterator walks every member");

  const scores = new Map();
  for await (const [member, score] of r.zscanIterator("bigzset", { count: 10 })) scores.set(member, score);
  is(scores.size, 300, "zscanIterator walks every member");
  is(scores.get("z42"), 42, "with numeric scores");

  // MATCH narrows without changing the walk.
  let matched = 0;
  for await (const _ of r.sscanIterator("bigset", { match: "m1?", count: 10 })) matched++;
  is(matched, 10, "MATCH narrows an iterator (m10–m19)");
}

// -- popping several --------------------------------------------------------

{
  await r.del("popme");
  await r.rpush("popme", "a", "b", "c", "d");
  is(await r.lpop("popme"), "a", "LPOP with no count answers a value");
  is(await r.lpop("popme", 2), ["b", "c"], "and with a count answers an array");
  is(await r.rpop("popme", 5), ["d"], "a count larger than the list gives what there is");
  is(await r.lpop("popme", 2), null, "and an empty list answers null, not an empty array");
}

// -- odds and ends ----------------------------------------------------------

{
  await r.del("l");
  await r.rpush("l", "a", "b", "c", "b");
  is(await r.lpos("l", "b"), 1, "LPOS finds the first match");
  is(await r.lpos("l", "b", { rank: 2 }), 3, "RANK finds the second");
  is(await r.lpos("l", "zzz"), null, "and null when there is none");
}

{
  await r.del("s1s", "s2s");
  await r.sadd("s1s", "a", "b", "c");
  await r.sadd("s2s", "b", "c", "d");
  is(await r.sintercard(["s1s", "s2s"]), 2, "SINTERCARD counts without building");
  is(await r.sintercard(["s1s", "s2s"], { limit: 1 }), 1, "and stops at a limit");
}

{
  await r.del("z", "zdest");
  await r.zadd("z", { a: 1, b: 2, c: 3 });
  is(await r.zmscore("z", "a", "c", "nope"), [1, 3, null], "ZMSCORE, with null for a missing member");
  is(await r.zrangestore("zdest", "z", 0, 1), 2, "ZRANGESTORE answers how many it stored");
  is(await r.zrange("zdest", 0, -1), ["a", "b"], "and stored the right ones");
}

// -- hash-field TTLs (Redis 7.4+) -------------------------------------------

if (version >= 7) {
  await r.del("h");
  await r.hset("h", { keep: "1", expire: "2" });
  is(await r.hexpire("h", 100, "expire"), [1], "HEXPIRE sets a TTL on one field");
  is(await r.hexpire("h", 100, "absent"), [-2], "and reports -2 for a field that is not there");

  const ttls = await r.httl("h", "expire", "keep");
  ok(ttls[0] > 90, "HTTL reports the field's remaining seconds");
  is(ttls[1], -1, "and -1 for a field with no expiry");

  is(await r.hpersist("h", "expire"), [1], "HPERSIST clears it");
  is(await r.httl("h", "expire"), [-1], "leaving no expiry");

  // The whole point: one field expires and the rest of the hash stays.
  await r.hpexpire("h", 50, "expire");
  await new Promise((resolve) => setTimeout(resolve, 200));
  is(await r.hget("h", "expire"), null, "the expired field is gone");
  is(await r.hget("h", "keep"), "1", "and the others are untouched");
} else {
  console.log("    (hash-field TTLs need Redis 7.4+; server is", version, ")");
}

await r.flushdb();
await r.close();
if (report("commands") > 0) exit(1);
