// Bun + its built-in client.
//
// Bun's own `RedisClient` rather than ioredis, because that is what a Bun user
// gets and it is the strongest form of the comparison — the question these
// benchmarks answer is what each runtime gives you, not how one npm package
// performs on four engines. Set `BUN_REDIS=ioredis` to compare like for like
// against Node instead.
import * as w from "./workload.mjs";

const workload = process.argv[2];
const url = process.env.REDIS_BENCH_URL ?? "redis://127.0.0.1:6379";
const useIoredis = process.env.BUN_REDIS === "ioredis";

let r;
if (useIoredis) {
  const { default: Redis } = await import("ioredis");
  r = new Redis(url, { enableAutoPipelining: false, maxRetriesPerRequest: null });
} else {
  const { RedisClient } = await import("bun");
  r = new RedisClient(url);
}

if (workload === "serial_set") {
  for (let i = 0; i < w.SERIAL; i++) await r.set(w.keyOf(i), w.VALUE);
  console.log(String(w.SERIAL));
} else if (workload === "serial_get") {
  let n = 0;
  for (let i = 0; i < w.SERIAL; i++) n += (await r.get(w.keyOf(i))).length;
  console.log(String(n));
} else if (workload === "pipeline") {
  if (useIoredis) {
    const p = r.pipeline();
    for (let i = 0; i < w.PIPELINE; i++) p.set(w.keyOf(i), w.VALUE);
    console.log(String((await p.exec()).length));
  } else {
    // Bun's client has no pipeline builder: it pipelines commands issued
    // together, so `Promise.all` is the idiom a Bun user would reach for and
    // the fair thing to measure.
    const pending = [];
    for (let i = 0; i < w.PIPELINE; i++) pending.push(r.set(w.keyOf(i), w.VALUE));
    console.log(String((await Promise.all(pending)).length));
  }
} else if (workload === "list") {
  const items = useIoredis
    ? await r.lrange(w.LIST_KEY, 0, -1)
    : await r.send("LRANGE", [w.LIST_KEY, "0", "-1"]);
  let n = 0;
  for (const item of items) n += item.length;
  console.log(String(n));
} else if (workload === "hash") {
  let n = 0;
  for (let i = 0; i < w.HASH_REPEATS; i++) {
    const all = useIoredis ? await r.hgetall(w.HASH_KEY) : await r.send("HGETALL", [w.HASH_KEY]);
    for (const value of Object.values(all)) n += value.length;
  }
  console.log(String(n));
}

if (useIoredis) await r.quit();
else r.close();
