// esrun + @opentf/esrun-postgres.
import { args, env } from "runtime:process";
// Staged into the bench tree by run.sh. The module loader is jailed to the
// project root it detects from the entry file — `bench/`, which has its own
// package.json — so a reach up into `packages/` is refused, correctly. Copying
// the built driver in is the honest way across that line.
import { connect } from "runtime:db";
import { driver as postgres } from "./.driver/index.js";
import * as w from "./workload.mjs";

const workload = args[0];
const db = await connect(env.PG_URL, { driver: postgres });

if (workload === "setup") {
  await db.executeScript(w.SCHEMA);
  console.log("ok");
} else if (workload === "scan_num") {
  let a = 0, b = 0n, c = 0, d = 0, e = 0;
  for await (const r of await db.query(w.SCAN_NUM)) {
    a += r.a;
    b += BigInt(r.b);
    c += r.c;
    d += r.d;
    e += r.e;
  }
  console.log(`${a}/${b}/${c.toFixed(1)}/${d.toFixed(2)}/${e}`);
} else if (workload === "scan_text") {
  let n = 0;
  for await (const r of await db.query(w.SCAN_TEXT)) n += r.s1.length + r.s2.length + r.s3.length;
  console.log(n);
} else if (workload === "small") {
  let n = 0;
  for (let i = 0; i < w.SMALL_REPEATS; i++) {
    n += (await (await db.query(w.SMALL)).toArray()).length;
  }
  console.log(n);
} else if (workload === "stream") {
  let n = 0;
  for await (const r of await db.query(w.STREAM)) n += r.a === undefined ? 0 : 1;
  console.log(n);
}
await db.close();
