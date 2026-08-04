// Error benchmark: throwing, catching, and reading a stack across a few frames.
// Servers throw on every rejected request, and capturing a stack is the
// expensive half — it walks frames and materialises strings, which is why this
// is separated from the throw/catch control flow rather than folded into it.
(async () => {
  const N = 100_000;

  const deep3 = (i) => {
    throw new Error("request failed: " + i);
  };
  const deep2 = (i) => deep3(i);
  const deep1 = (i) => deep2(i);

  const run = (n) => {
    let acc = 0;
    for (let i = 0; i < n; i++) {
      try {
        deep1(i);
      } catch (e) {
        acc += e.message.length;
        // Reading `.stack` is what forces the capture to be materialised;
        // without this the row would measure only the unwind.
        acc += e.stack ? e.stack.length & 7 : 0;
      }
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
