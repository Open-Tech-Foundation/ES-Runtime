// Timer benchmark: schedule a burst of zero-delay timers and wait for all of
// them to fire. Measures timer scheduling + firing overhead (breadth, not a
// chain — chained setTimeout(0) mostly measures each runtime's minimum-delay
// clamp, not its bookkeeping).
// N is 100k rather than 10k because at 10k the fastest runtime finished in
// 2.7 ms — close enough to the timer's own resolution that the ranking was
// partly noise. The work is unchanged in kind, only in amount.
(async () => {
  const N = 100_000;
  const run = (n) =>
    new Promise((resolve) => {
      let done = 0;
      for (let i = 0; i < n; i++) {
        setTimeout(() => {
          if (++done === n) resolve();
        }, 0);
      }
    });
  await run(N / 10); // untimed warmup
  const t0 = performance.now();
  await run(N);
  const t1 = performance.now();
  console.log("RESULT_MS=" + (t1 - t0).toFixed(2));
})();
