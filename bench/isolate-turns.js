// What a CPU budget costs an isolate's calls (D237).
//
//   esrun --allow-workers bench/isolate-turns.js
//
// Every isolate turn carries a budget since D237, and the pool's watchdog is
// told of each one. Each row pair compares an isolate given no `cpuTime` with
// one given an explicit budget far larger than any turn here, on one isolate
// (round-trip latency) and on 24 at once (throughput across the pool). Best of
// three.
//
// Run against a build from before D237, where no `cpuTime` meant no budget, the
// pairs measure what a budget costs — the D237 figures. On a later build both
// rows of a pair are budgeted and should agree.
import { Isolate } from "runtime:workers";

const SOURCE = { source: "let n = 0; export const inc = () => ++n;" };

async function rate(options, isolates, calls) {
  const all = Array.from({ length: isolates }, () => new Isolate(SOURCE, options));
  const fns = await Promise.all(all.map(async (iso) => (await iso.exports).inc));
  for (const inc of fns) for (let k = 0; k < 200; k++) await inc();
  const start = performance.now();
  await Promise.all(
    fns.map(async (inc) => {
      for (let k = 0; k < calls; k++) await inc();
    }),
  );
  const ms = performance.now() - start;
  for (const iso of all) iso.terminate();
  return { perSecond: (isolates * calls) / (ms / 1000), usPerCall: (ms * 1000) / calls };
}

async function best(options, isolates, calls) {
  let fastest = null;
  for (let round = 0; round < 3; round++) {
    const r = await rate(options, isolates, calls);
    if (fastest === null || r.perSecond > fastest.perSecond) fastest = r;
  }
  return fastest;
}

const rows = [
  ["1 isolate, default budget", {}, 1, 20000],
  ["1 isolate, cpuTime 3600000", { cpuTime: 3_600_000 }, 1, 20000],
  ["24 isolates, default budget", {}, 24, 3000],
  ["24 isolates, cpuTime 3600000", { cpuTime: 3_600_000 }, 24, 3000],
];
for (const [label, options, isolates, calls] of rows) {
  const r = await best(options, isolates, calls);
  console.log(
    `${label.padEnd(30)} ${(r.perSecond / 1000).toFixed(0).padStart(5)}k calls/s  ` +
      `${r.usPerCall.toFixed(2).padStart(7)} µs a call per isolate`,
  );
}
