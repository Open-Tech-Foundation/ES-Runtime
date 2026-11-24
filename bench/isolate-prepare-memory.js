// What a prepared isolate costs in resident memory (D250).
//
//   esrun --allow-workers --allow-diagnostics bench/isolate-prepare-memory.js
//
// Prepares isolates in steps and reports the process's resident set after each,
// and the cost of each step's isolates divided by their number. Then claims
// some of them with `new Isolate()` — what a prepared isolate becomes when used —
// and reports that too. RSS is the process's (Linux: /proc/self/status), read
// through `metrics()`; it includes the pool's threads, which the first step pays.
import { metrics } from "runtime:diagnostics";
import { Isolate } from "runtime:workers";

const MB = 1024 * 1024;
const rss = () => metrics().process.rss;
const settle = () => new Promise((r) => setTimeout(r, 300));

await settle();
const base = rss();
console.log(`baseline              rss ${(base / MB).toFixed(1)} MB`);

let prepared = 0;
let last = base;
for (const step of [10, 50, 100, 200]) {
  // `prepare(n)` builds n more and resolves with how many now wait.
  prepared = await Isolate.prepare(step);
  await settle();
  const now = rss();
  console.log(
    `prepared ${String(prepared).padStart(4)}         rss ${(now / MB).toFixed(1).padStart(7)} MB   ` +
      `${((now - last) / step / MB).toFixed(2)} MB each for these ${step}`,
  );
  last = now;
}

const claimed = Array.from({ length: 50 }, () => new Isolate({ source: "export const id = (x) => x;" }));
await Promise.all(claimed.map(async (iso) => (await iso.exports).id(1)));
await settle();
const used = rss();
console.log(
  `claimed 50, called     rss ${(used / MB).toFixed(1).padStart(7)} MB   ` +
    `${((used - last) / 50 / MB).toFixed(2)} MB more each than prepared`,
);
for (const iso of claimed) iso.terminate();
