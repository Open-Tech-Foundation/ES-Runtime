// Node.js / Deno + ioredis.
//
// ioredis rather than a runtime built-in, because neither Node nor Deno has
// one — which is the comparison, not a handicap. Each workload uses the idiom
// an ioredis user would reach for: `.pipeline()` for the batch, plain awaits
// for the serial shapes.
import Redis from "ioredis";
import * as w from "./workload.mjs";

const workload = process.argv[2];
const url = process.env.REDIS_BENCH_URL ?? "redis://127.0.0.1:6379";
// ioredis pipelines automatically inside one tick, which would make the
// "serial" workloads measure something other than round trips. Off, so serial
// means serial for everybody.
const r = new Redis(url, { enableAutoPipelining: false, maxRetriesPerRequest: null });

if (workload === "serial_set") {
  for (let i = 0; i < w.SERIAL; i++) await r.set(w.keyOf(i), w.VALUE);
  console.log(String(w.SERIAL));
} else if (workload === "serial_get") {
  let n = 0;
  for (let i = 0; i < w.SERIAL; i++) n += (await r.get(w.keyOf(i))).length;
  console.log(String(n));
} else if (workload === "pipeline") {
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

await r.quit();
