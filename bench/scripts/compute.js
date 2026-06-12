// Compute benchmark: a CPU-bound numeric loop. Mostly measures the JS engine
// (V8 for esrun/Node/Deno; JavaScriptCore for Bun) and call overhead.
(async () => {
  const ITERS = 20_000_000;
  const t0 = performance.now();
  let acc = 0;
  for (let i = 1; i < ITERS; i++) {
    acc += Math.sqrt(i) - Math.log(i) * 0.5;
  }
  const t1 = performance.now();
  if (acc === -1) console.log(acc); // defeat dead-code elimination
  console.log("RESULT_MS=" + (t1 - t0).toFixed(2));
})();
