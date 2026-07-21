// WASM compile benchmark: 60 × `WebAssembly.compile` of a ~250 KB module.
// Measures validation + baseline codegen — the cost of loading a wasm payload,
// which for esrun is V8's own compiler reached through the WebAssembly JS API.
//
// Each module carries a different salt, so the bytes differ every iteration and
// no runtime can serve the result from a compilation cache.
(async () => {
  if (typeof WebAssembly === "undefined") return; // no wasm engine (n/a)
  const { bigModule } = await import("./wasm-mod.js");

  const ITERS = 60;
  const shape = { funcs: 600, chain: 60 };
  const modules = [];
  for (let i = 0; i < ITERS; i++) modules.push(bigModule({ ...shape, salt: i * 1000 }));

  await WebAssembly.compile(bigModule({ ...shape, salt: -1 })); // untimed warmup
  const t0 = performance.now();
  let exports = 0;
  for (const bytes of modules) {
    exports += WebAssembly.Module.exports(await WebAssembly.compile(bytes)).length;
  }
  const t1 = performance.now();
  if (exports < 0) console.log(exports); // defeat dead-code elimination
  console.log("RESULT_MS=" + (t1 - t0).toFixed(2));
})();
