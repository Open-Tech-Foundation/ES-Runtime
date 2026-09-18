// Postgres QPS on Node: postgres.js, pool of 100, 100 workers in flight.
import postgres from "postgres";
import { QPS_QUERY, QPS_ROWS, QPS_TOTAL, QPS_WORKERS, expectedSum } from "./qps-shared.mjs";

const WARMUP_S = Number(process.env.QPS_WARMUP ?? 3);
const sql = postgres(process.env.PG_URL, { prepare: true, fetch_types: false, max: 100 });

async function one() {
  const rows = await sql.unsafe(QPS_QUERY);
  if (rows.length !== QPS_ROWS) throw new Error(`expected ${QPS_ROWS} rows, got ${rows.length}`);
  return rows;
}

let sum = 0;
for (const r of await one()) sum += r.a;
if (sum !== expectedSum()) throw new Error(`checksum mismatch: ${sum}`);

async function hammer(seconds) {
  const end = Date.now() + seconds * 1000;
  let n = 0;
  const workers = [];
  for (let k = 0; k < QPS_WORKERS; k++) {
    workers.push((async () => {
      while (Date.now() < end) {
        await one();
        n++;
      }
    })());
  }
  await Promise.all(workers);
  return n;
}

await hammer(WARMUP_S);
const t0 = Date.now();
let promises = [];
for (let i = 0; i < QPS_TOTAL; i++) {
  promises.push(one());
  if (i % QPS_WORKERS === 0 && promises.length > 1) {
    await Promise.all(promises);
    promises.length = 0;
  }
}
await Promise.all(promises);
const seconds = (Date.now() - t0) / 1000;
console.log(JSON.stringify({ queries: QPS_TOTAL, seconds, qps: Math.round(QPS_TOTAL / seconds) }));
await sql.end();
