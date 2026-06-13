// Base64 benchmark: btoa/atob round trips of a 1 KiB latin1 string. For esrun
// this is the pure-JS base64 prelude; the others are native.
(async () => {
  const s = "abcdefgh".repeat(128); // 1 KiB, latin1-safe
  const N = 10_000;
  const run = (n) => {
    let acc = 0;
    for (let i = 0; i < n; i++) {
      acc += atob(btoa(s)).length;
    }
    return acc;
  };
  run(N / 10); // untimed JIT warmup
  const t0 = performance.now();
  const acc = run(N);
  const t1 = performance.now();
  if (acc === -1) console.log(acc); // defeat dead-code elimination
  console.log("RESULT_MS=" + (t1 - t0).toFixed(2));
})();
