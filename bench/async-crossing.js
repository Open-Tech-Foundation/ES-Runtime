// What one async op round trip costs: `await` a file stat, which does almost
// no work of its own, so the time is the crossing to the host and back.
//
//   esrun --allow-read bench/async-crossing.js
//   node bench/async-crossing.js
//
// The figure `runtime:db`'s design rests on (internals/database, the driver
// authoring guide): anything that crosses once per row pays it per row. Best
// of five runs of 20 000 sequential stats of this file.
const isEsrun = typeof globalThis.process === "undefined";
const { stat } = isEsrun ? await import("runtime:fs") : await import("node:fs/promises");
const path = new URL(import.meta.url).pathname; // this file, on either runtime
const N = 20_000;
for (let i = 0; i < 2_000; i++) await stat(path);
let best = Infinity;
for (let round = 0; round < 5; round++) {
  const t0 = performance.now();
  for (let i = 0; i < N; i++) await stat(path);
  best = Math.min(best, ((performance.now() - t0) * 1000) / N);
}
console.log(`${isEsrun ? "esrun" : `node ${process.version}`}  await stat()  ${best.toFixed(1)} µs a call`);
