// URL setter benchmark: WHATWG URL parsing + component sets in a loop.
(async () => {
  const N = 100_000;
  const run = (n) => {
    let acc = 0;
    const u = new URL("https://example.com/a/b?q=0&lang=en#frag");
    for (let i = 0; i < n; i++) {
      u.hostname = "test" + i + ".com";
      u.host = "test" + i + ".com:8080";
      acc += u.hostname.length;
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
