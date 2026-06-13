// Async-overhead benchmark: awaiting already-resolved promises in a tight
// loop. Measures the microtask/promise machinery (and for esrun the
// microtask-checkpoint integration of the driven loop) — no timers, no I/O.
(async () => {
  const N = 1_000_000;
  const run = async (n) => {
    let acc = 0;
    for (let i = 0; i < n; i++) {
      acc += await Promise.resolve(i & 7);
    }
    return acc;
  };
  await run(N / 10); // untimed JIT warmup
  const t0 = performance.now();
  const acc = await run(N);
  const t1 = performance.now();
  if (acc === -1) console.log(acc); // defeat dead-code elimination
  console.log("RESULT_MS=" + (t1 - t0).toFixed(2));
})();
