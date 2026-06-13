// Encoding benchmark: TextEncoder/TextDecoder UTF-8 round trips. For esrun each
// call is one JS↔Rust op crossing riding V8's native UTF-16↔UTF-8 conversion;
// the others transcode natively.
(async () => {
  const enc = new TextEncoder();
  const dec = new TextDecoder();
  const N = 100_000;
  const run = (n) => {
    let acc = 0;
    for (let i = 0; i < n; i++) {
      const bytes = enc.encode("/a/b?q=" + i + "&lang=en");
      acc += dec.decode(bytes).length;
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
