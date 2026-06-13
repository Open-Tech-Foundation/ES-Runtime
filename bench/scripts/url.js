// URL benchmark: WHATWG URL parsing + component reads in a loop. For esrun each
// parse is one JS↔Rust op crossing returning href + component offsets; Node/
// Deno/Bun parse natively (Ada / native engine code).
(async () => {
  const N = 100_000;
  const run = (n) => {
    let acc = 0;
    for (let i = 0; i < n; i++) {
      const u = new URL("https://example.com/a/b?q=" + i + "&lang=en#frag");
      acc += u.pathname.length + u.search.length;
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
