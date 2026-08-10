// esrun — `runtime:db`, async over the op boundary.
import { connect, sqlite } from "runtime:db";
import { args } from "runtime:process";
import * as w from "./workload.mjs";

const [workload, path] = args;
const db = await connect(`sqlite:${path}`, { driver: sqlite });

if (workload === "open") {
  await db.execute(w.SCHEMA);
  console.log("ok");
} else if (workload === "insert") {
  await db.execute(w.SCHEMA);
  await db.transaction(async (tx) => {
    for (let i = 0; i < w.ROWS; i++) await tx.execute(w.INSERT, w.row(i));
  });
  console.log((await (await db.query("SELECT count(*) AS n FROM items")).first()).n);
} else if (workload === "insert_many") {
  // The batched path: one crossing and one prepare for the whole run, against
  // the per-statement loop above. Node and Bun have no equivalent — their
  // SQLite APIs are synchronous, so a loop costs them a function call rather
  // than a boundary.
  await db.execute(w.SCHEMA);
  const rows = [];
  for (let i = 0; i < w.ROWS; i++) rows.push(w.row(i));
  await db.executeMany(w.INSERT, rows);
  console.log((await (await db.query("SELECT count(*) AS n FROM items")).first()).n);
} else if (workload === "scan_num") {
  let a = 0, b = 0, c = 0;
  for await (const r of await db.query(w.SCAN_NUM)) {
    a += r.a;
    b += r.b;
    c += r.c;
  }
  console.log(`${a}/${b}/${c.toFixed(1)}`);
} else if (workload === "scan_text") {
  let n = 0;
  for await (const r of await db.query(w.SCAN_TEXT)) n += r.label.length + r.body.length;
  console.log(n);
} else if (workload === "point") {
  let sum = 0;
  for (let i = 0; i < w.POINTS; i++) {
    const r = await (await db.query(w.POINT, [w.pointId(i)])).first();
    sum += r.a + r.b;
  }
  console.log(sum);
} else if (workload === "stream") {
  let n = 0;
  for await (const r of await db.query(w.STREAM)) n += r.a === undefined ? 0 : 1;
  console.log(n);
} else if (workload === "seed_stream") {
  await db.execute(w.SCHEMA);
  await db.transaction(async (tx) => {
    for (let i = 0; i < w.STREAM_ROWS; i++) await tx.execute(w.INSERT, w.row(i));
  });
  console.log("ok");
}
await db.close();
