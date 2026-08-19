// Durable workers: what a call costs, and what the gate costs (DECISIONS D80).
//
// Run:  esrun --allow-read --allow-write bench/durable-workers.js
//       (from a directory it may write to — it writes ./.bench-durable and
//        removes it on the way out.)
//
// Why this exists: every number the documentation quotes about durable workers
// has to come from somewhere a reader can re-run. Three of them matter.
//
//   * **A read is a map lookup.** State is resident, so `state.get` should cost
//     what a `Map` costs and nothing else. If this row ever approaches the
//     write row, the residency claim has quietly stopped being true.
//   * **A gated write is a commit.** A call that writes cannot return before
//     its transaction has committed, so this row *is* SQLite's fsync-bounded
//     write rate. It is the number to weigh against "just use a variable".
//   * **A batch is one commit.** `setMany` exists because a per-key commit
//     spends its time on the boundary rather than in the database.
//
// Also here: materialization, which is what an idle worker's first call pays.
import { DurableWorker, configure, shutdown } from "runtime:workers";
import { remove } from "runtime:fs";

const DIR = "./.bench-durable";
configure({ dir: DIR, evictAfter: 50 });

class Bench extends DurableWorker {
  async read(key) {
    return this.state.get(key);
  }
  async write(key, value) {
    this.state.set(key, value);
    return 1;
  }
  async writeMany(entries) {
    this.state.setMany(entries);
    return entries.size;
  }
  async nothing() {
    return 0;
  }
}

const report = (label, ops, ms) =>
  console.log(
    `${label.padEnd(34)} ${((ops / ms) * 1000).toFixed(0).padStart(9)} /s   ` +
      `${((ms * 1000) / ops).toFixed(1).padStart(8)} µs/op`,
  );

async function time(label, ops, run) {
  const started = performance.now();
  await run();
  report(label, ops, performance.now() - started);
}

const w = Bench.get("bench");
await w.write("seed", { n: 1 });

const CALLS = 2000;
await time("call, no state touched", CALLS, async () => {
  for (let i = 0; i < CALLS; i++) await w.nothing();
});

await time("call, one resident read", CALLS, async () => {
  for (let i = 0; i < CALLS; i++) await w.read("seed");
});

const WRITES = 500;
await time("call, one gated write", WRITES, async () => {
  for (let i = 0; i < WRITES; i++) await w.write("k", i);
});

const BATCH = 100;
const batches = 20;
await time("setMany, 100 keys per commit", BATCH * batches, async () => {
  for (let b = 0; b < batches; b++) {
    const entries = new Map();
    for (let i = 0; i < BATCH; i++) entries.set(`b${b}:${i}`, i);
    await w.writeMany(entries);
  }
});

// Materialization: a worker nobody has addressed yet, opened and read for the
// first time. This is what the first call after an idle window pays.
const COLD = 100;
await time("first call on a new worker", COLD, async () => {
  for (let i = 0; i < COLD; i++) await Bench.get(`cold-${i}`).nothing();
});

await shutdown();
await remove(DIR, { recursive: true });
