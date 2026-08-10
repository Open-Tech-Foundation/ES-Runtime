// How much of a scan is decoding?
//
// The question that decides whether binary result formats are worth their
// complexity, and it has an exact answer here: rows are lazy, so a column
// nobody reads is never decoded. Running the same query while touching a
// different number of columns holds the network and the protocol constant and
// varies only the decoding.
//
//   PG_URL=… esrun bench/db/pg/decode-share.mjs      (after run.sh has seeded)
import { env } from "runtime:process";
import { connect } from "./.driver/index.js";
const db = await connect(env.PG_URL);
const N = 10000;
const SQL = `SELECT a, b, c, d, e FROM bench_num WHERE id <= ${N}`;
const time = async (label, touch) => {
  // Three runs, keep the fastest — the same rule the harness uses.
  let best = Infinity;
  for (let r = 0; r < 3; r++) {
    const s = performance.now();
    let sink = 0;
    for await (const row of await db.query(SQL)) sink += touch(row);
    const ms = performance.now() - s;
    if (ms < best) best = ms;
    void sink;
  }
  console.log(`${label}: ${best.toFixed(1)}ms  (${((best * 1000) / N).toFixed(2)}us/row)`);
  return best;
};

// Rows are lazy: a column nobody reads is never decoded. So the gap between
// these is the decoding, with the network and the protocol held constant.
const none = await time("touch 0 columns ", () => 0);
const one = await time("touch 1 (int4)  ", (r) => r.a);
const all = await time("touch 5 columns ", (r) => r.a + Number(r.b) + r.c + r.d + r.e);
const big = await time("touch int8 only ", (r) => Number(r.b));
console.log(`decoding is ${(((all - none) / all) * 100).toFixed(0)}% of the query`);
await db.close();
import { env } from "runtime:process";
import { connect } from "./.driver/index.js";
const db = await connect(env.PG_URL);
const N = 10000;
const SQL = `SELECT a, b, c, d, e FROM bench_num WHERE id <= ${N}`;
const time = async (label, touch) => {
  // Three runs, keep the fastest — the same rule the harness uses.
  let best = Infinity;
  for (let r = 0; r < 3; r++) {
    const s = performance.now();
    let sink = 0;
    for await (const row of await db.query(SQL)) sink += touch(row);
    const ms = performance.now() - s;
    if (ms < best) best = ms;
    void sink;
  }
  console.log(`${label}: ${best.toFixed(1)}ms  (${((best * 1000) / N).toFixed(2)}us/row)`);
  return best;
};

// Rows are lazy: a column nobody reads is never decoded. So the gap between
// these is the decoding, with the network and the protocol held constant.
const none = await time("touch 0 columns ", () => 0);
const one = await time("touch 1 (int4)  ", (r) => r.a);
const all = await time("touch 5 columns ", (r) => r.a + Number(r.b) + r.c + r.d + r.e);
const big = await time("touch int8 only ", (r) => Number(r.b));
console.log(`decoding is ${(((all - none) / all) * 100).toFixed(0)}% of the query`);
await db.close();
