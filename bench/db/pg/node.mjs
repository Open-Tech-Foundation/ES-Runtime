// Node.js / Bun / Deno + postgres.js.
//
// Each workload uses the idiom a postgres.js user would reach for: the default
// buffering path for a scan, and `.cursor()` only for the streaming workload,
// where the point is that memory must not grow with the result. Giving it a
// cursor for the scans would charge it a round trip per batch it never asked
// for.
import postgres from "postgres";
import * as w from "./workload.mjs";

const workload = process.argv[2];
// `fetch_types: false` skips postgres.js's type-catalogue query at startup,
// which is setup cost rather than query cost and would only distort a short run.
const sql = postgres(process.env.PG_URL, { prepare: true, fetch_types: false });

if (workload === "setup") {
  await sql.unsafe(w.SCHEMA);
  console.log("ok");
} else if (workload === "scan_num") {
  let a = 0, b = 0n, c = 0, d = 0, e = 0;
  for (const r of await sql.unsafe(w.SCAN_NUM)) {
    a += r.a;
    b += BigInt(r.b);
    c += r.c;
    d += r.d;
    e += r.e;
  }
  console.log(`${a}/${b}/${c.toFixed(1)}/${d.toFixed(2)}/${e}`);
} else if (workload === "scan_text") {
  let n = 0;
  for (const r of await sql.unsafe(w.SCAN_TEXT)) n += r.s1.length + r.s2.length + r.s3.length;
  console.log(n);
} else if (workload === "small") {
  let n = 0;
  for (let i = 0; i < w.SMALL_REPEATS; i++) n += (await sql.unsafe(w.SMALL)).length;
  console.log(n);
} else if (workload === "stream") {
  let n = 0;
  for await (const rows of sql.unsafe(w.STREAM).cursor(2000)) {
    for (const r of rows) n += r.a === undefined ? 0 : 1;
  }
  console.log(n);
}
await sql.end();
