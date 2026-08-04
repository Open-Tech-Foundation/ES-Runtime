// Date + Intl benchmark: formatting a timestamp and a number through cached
// `Intl` formatters, plus the ISO path. Intl is backed by ICU, which is one of
// the larger things a runtime chooses to bundle or omit — it shows up in binary
// size and startup, and a runtime built without it reports n/a here rather than
// a number. Formatters are constructed once, outside the loop, because
// constructing them per call measures formatter setup instead of formatting.
(async () => {
  const dtf = new Intl.DateTimeFormat("en-US", {
    year: "numeric",
    month: "short",
    day: "2-digit",
    hour: "2-digit",
    minute: "2-digit",
    timeZone: "UTC",
  });
  const nf = new Intl.NumberFormat("en-US", {
    style: "currency",
    currency: "USD",
  });

  const N = 50_000;
  const run = (n) => {
    let acc = 0;
    for (let i = 0; i < n; i++) {
      const d = new Date(1_700_000_000_000 + i * 1000);
      acc += dtf.format(d).length;
      acc += nf.format(i * 1.37).length;
      acc += d.toISOString().length;
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
