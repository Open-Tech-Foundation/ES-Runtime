// What an isolate takes to start, cold and from Isolate.prepare (D227).
//
//   esrun --allow-workers bench/isolate-start.js
//
// Each row starts N isolates at once, waits until every one serves its
// exports, and reports the wall time per isolate (best of five). "cold" builds
// each from the snapshot as it is asked for; "prepared" first calls
// Isolate.prepare(N), untimed, so the timed starts claim isolates already
// built. Per-isolate times are amortised over the pool's threads.
import { Isolate } from "runtime:workers";

const N = 200;
const SOURCE = { source: "export const id = (k) => k;" };

async function startAll() {
  const start = performance.now();
  const all = Array.from({ length: N }, () => new Isolate(SOURCE));
  await Promise.all(all.map((iso) => iso.exports));
  const ms = performance.now() - start;
  for (const iso of all) iso.terminate();
  return ms / N;
}

async function best(prepare) {
  let fastest = Infinity;
  for (let round = 0; round < 5; round++) {
    if (prepare) await Isolate.prepare(N);
    fastest = Math.min(fastest, await startAll());
  }
  return fastest;
}

const cold = await best(false);
const warm = await best(true);
console.log(`cold      ${(cold * 1000).toFixed(0).padStart(6)} µs/isolate`);
console.log(`prepared  ${(warm * 1000).toFixed(0).padStart(6)} µs/isolate`);
