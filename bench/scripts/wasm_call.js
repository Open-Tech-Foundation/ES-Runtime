// WASM call benchmark: 20M calls across the JS↔wasm boundary into a trivial
// exported `add`, then the same arithmetic done entirely inside wasm. The pair
// separates per-call boundary cost from wasm execution itself.
(async () => {
  if (typeof WebAssembly === "undefined") return; // no wasm engine (n/a)
  const { computeModule } = await import("./wasm-mod.js");

  const CALLS = 20_000_000;
  const INNER = 100_000_000;
  const { instance } = await WebAssembly.instantiate(computeModule(), {});
  const { add, sum } = instance.exports;

  const cross = (n) => {
    let acc = 0;
    for (let i = 0; i < n; i++) acc = add(acc, i) | 0;
    return acc;
  };

  cross(CALLS / 10); // untimed JIT warmup
  sum(INNER / 10);
  const t0 = performance.now();
  const acc = cross(CALLS) + sum(INNER);
  const t1 = performance.now();
  if (acc === -1) console.log(acc); // defeat dead-code elimination
  console.log("RESULT_MS=" + (t1 - t0).toFixed(2));
})();
