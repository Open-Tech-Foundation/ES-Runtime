// Large-document JSON benchmark: parse + re-stringify one ~5 MB document a few
// times. Complements json.js (many small objects): this shape is dominated by
// allocation throughput and GC behaviour rather than per-call overhead.
(async () => {
  const doc = { items: [] };
  for (let i = 0; i < 50_000; i++) {
    doc.items.push({
      id: i,
      name: "item-" + i,
      tags: ["red", "green", "blue"],
      price: i * 1.01,
      active: (i & 1) === 0,
    });
  }
  const str = JSON.stringify(doc); // ~5 MB
  const N = 15;
  const run = (n) => {
    let acc = 0;
    for (let i = 0; i < n; i++) {
      acc += JSON.stringify(JSON.parse(str)).length;
    }
    return acc;
  };
  run(2); // untimed warmup
  const t0 = performance.now();
  const acc = run(N);
  const t1 = performance.now();
  if (acc === -1) console.log(acc); // defeat dead-code elimination
  console.log("RESULT_MS=" + (t1 - t0).toFixed(2));
})();
