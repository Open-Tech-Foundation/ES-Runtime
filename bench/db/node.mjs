// Node.js — `node:sqlite` (DatabaseSync), synchronous and in-process.
import { DatabaseSync } from "node:sqlite";
import * as w from "./workload.mjs";

const [workload, path] = process.argv.slice(2);
const db = new DatabaseSync(path);

if (workload === "open") {
  db.exec(w.SCHEMA);
  console.log("ok");
} else if (workload === "insert" || workload === "seed_stream") {
  const rows = workload === "insert" ? w.ROWS : w.STREAM_ROWS;
  db.exec(w.SCHEMA);
  const stmt = db.prepare(w.INSERT);
  db.exec("BEGIN");
  for (let i = 0; i < rows; i++) stmt.run(...w.row(i));
  db.exec("COMMIT");
  console.log(workload === "insert" ? db.prepare("SELECT count(*) AS n FROM items").get().n : "ok");
} else if (workload === "insert_many") {
  // No batch API: this runtime's SQLite binding is synchronous, so a prepared
  // statement in a loop is already its fastest path — measured as `insert`.
  console.log("n/a");
} else if (workload === "scan_num") {
  let a = 0, b = 0, c = 0;
  for (const r of db.prepare(w.SCAN_NUM).iterate()) {
    a += r.a;
    b += r.b;
    c += r.c;
  }
  console.log(`${a}/${b}/${c.toFixed(1)}`);
} else if (workload === "scan_text") {
  let n = 0;
  for (const r of db.prepare(w.SCAN_TEXT).iterate()) n += r.label.length + r.body.length;
  console.log(n);
} else if (workload === "point") {
  const stmt = db.prepare(w.POINT);
  let sum = 0;
  for (let i = 0; i < w.POINTS; i++) {
    const r = stmt.get(w.pointId(i));
    sum += r.a + r.b;
  }
  console.log(sum);
} else if (workload === "stream") {
  let n = 0;
  for (const r of db.prepare(w.STREAM).iterate()) n += r.a === undefined ? 0 : 1;
  console.log(n);
}
db.close();
