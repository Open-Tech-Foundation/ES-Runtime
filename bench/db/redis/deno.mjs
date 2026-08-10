// Deno + node-redis, through the npm: specifier.
//
// The official client, maintained by Redis in `redis/node-redis`, and the
// default here for that reason. `ioredis` gets its own column rather than
// standing in for Node: it has roughly twice the downloads but negotiates
// RESP2 where this negotiates RESP3, and the two do not perform alike.
//
// node-redis has no pipeline builder — commands issued together in one tick are
// pipelined automatically — so `Promise.all` is the idiom a node-redis user
// would reach for, and the fair thing to measure.
import { createClient } from "npm:redis";
import * as w from "./workload.mjs";

const workload = process.argv[2];
const url = process.env.REDIS_BENCH_URL ?? "redis://127.0.0.1:6379";
const r = createClient({ url });
await r.connect();

if (workload === "serial_set") {
  for (let i = 0; i < w.SERIAL; i++) await r.set(w.keyOf(i), w.VALUE);
  console.log(String(w.SERIAL));
} else if (workload === "serial_get") {
  let n = 0;
  for (let i = 0; i < w.SERIAL; i++) n += (await r.get(w.keyOf(i))).length;
  console.log(String(n));
} else if (workload === "pipeline") {
  const pending = [];
  for (let i = 0; i < w.PIPELINE; i++) pending.push(r.set(w.keyOf(i), w.VALUE));
  console.log(String((await Promise.all(pending)).length));
} else if (workload === "list") {
  const items = await r.lRange(w.LIST_KEY, 0, -1);
  let n = 0;
  for (const item of items) n += item.length;
  console.log(String(n));
} else if (workload === "hash") {
  let n = 0;
  for (let i = 0; i < w.HASH_REPEATS; i++) {
    const all = await r.hGetAll(w.HASH_KEY);
    for (const value of Object.values(all)) n += value.length;
  }
  console.log(String(n));
}

await r.quit();
