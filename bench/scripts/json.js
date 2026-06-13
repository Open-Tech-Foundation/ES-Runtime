// JSON benchmark: stringify+parse round trips of a small object. Pure engine
// work on every runtime (no host/op crossings) — a baseline that separates
// engine speed from runtime-layer cost in the other workloads.
(async () => {
  const N = 200_000;
  const run = (n) => {
    let acc = 0;
    for (let i = 0; i < n; i++) {
      const obj = {
        id: i,
        name: "user" + i,
        tags: ["alpha", "beta"],
        nested: { x: i * 1.5, ok: true },
      };
      acc += JSON.parse(JSON.stringify(obj)).id;
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
