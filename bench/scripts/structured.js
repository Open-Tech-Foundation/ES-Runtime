// structuredClone benchmark: deep-cloning a nested plain-data object. For
// esrun this is the pure-JS structured-clone prelude; the others are native
// (serializer-backed).
(async () => {
  const obj = {
    id: 7,
    name: "benchmark-object",
    tags: ["alpha", "beta", "gamma"],
    point: { x: 1.5, y: -2.5, z: 0 },
    rows: [
      { k: "a", v: 1, flags: [true, false] },
      { k: "b", v: 2, flags: [false, true] },
      { k: "c", v: 3, flags: [true, true] },
    ],
  };
  const N = 50_000;
  const run = (n) => {
    let acc = 0;
    for (let i = 0; i < n; i++) {
      acc += structuredClone(obj).rows.length;
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
