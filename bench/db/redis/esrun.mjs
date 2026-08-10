// esrun + @opentf/esrun-redis.
import { args, env } from "runtime:process";
// Staged into the bench tree by run.sh: the module loader is jailed to the
// project root it detects from the entry file — `bench/` — so a reach up into
// `packages/` is refused, correctly. Copying the built driver in is the honest
// way across that line, and it keeps the benchmark measuring the artifact that
// ships.
import { Redis } from "./.driver/index.js";
import * as w from "./workload.mjs";

const workload = args[0];
const r = await Redis.connect(env.REDIS_BENCH_URL ?? "redis://127.0.0.1:6379");

if (workload === "setup") {
  await r.call(["FLUSHDB"]);
  // The list, in batches so the setup itself is not the slow part.
  for (let start = 0; start < w.LIST_LEN; start += 5_000) {
    const chunk = [];
    for (let i = start; i < Math.min(start + 5_000, w.LIST_LEN); i++) chunk.push(`item-${i}`);
    await r.rpush(w.LIST_KEY, ...chunk);
  }
  const fields = {};
  for (let i = 0; i < w.HASH_FIELDS; i++) fields[`f${i}`] = w.fieldValue(i);
  await r.hset(w.HASH_KEY, fields);
  // The keys the read workloads want.
  const seed = r.pipeline();
  for (let i = 0; i < w.SERIAL; i++) seed.set(w.keyOf(i), w.VALUE);
  await seed.exec();
  console.log("ok");
} else if (workload === "serial_set") {
  for (let i = 0; i < w.SERIAL; i++) await r.set(w.keyOf(i), w.VALUE);
  console.log(String(w.SERIAL));
} else if (workload === "serial_get") {
  let n = 0;
  for (let i = 0; i < w.SERIAL; i++) n += (await r.get(w.keyOf(i))).length;
  console.log(String(n));
} else if (workload === "pipeline") {
  // One batch, one round trip. The builder is this driver's idiom for it.
  const p = r.pipeline();
  for (let i = 0; i < w.PIPELINE; i++) p.set(w.keyOf(i), w.VALUE);
  const results = await p.exec();
  console.log(String(results.length));
} else if (workload === "list") {
  const items = await r.lrange(w.LIST_KEY, 0, -1);
  let n = 0;
  for (const item of items) n += item.length;
  console.log(String(n));
} else if (workload === "hash") {
  let n = 0;
  for (let i = 0; i < w.HASH_REPEATS; i++) {
    const all = await r.hgetall(w.HASH_KEY);
    for (const value of Object.values(all)) n += value.length;
  }
  console.log(String(n));
}

await r.close();
