// WASI bootstrap benchmark: 2 000 × (construct a WASI, instantiate a command
// module against its import object, run `_start`). This is the cost of *running
// a wasm32-wasip1 program* — the path a CLI takes per invocation — with the
// module compiled once up front so compilation isn't counted twice.
//
// The guest makes no syscalls (see wasi_syscall for those): what is measured is
// building the preview-1 import object, binding memory, and the `_start` call.
(async () => {
  if (typeof WebAssembly === "undefined") return; // no wasm engine (n/a)
  const { wasiModule, loadWASI } = await import("./wasm-mod.js");
  const WASI = await loadWASI();
  if (!WASI) return; // runtime has no WASI (n/a)

  const RUNS = 2_000;
  const module = await WebAssembly.compile(wasiModule({ syscalls: 0 }));
  const options = { version: "preview1", args: ["prog", "--bench"], env: { LOG: "off" } };

  // A WASI instance may only be started once, so each run gets a fresh one —
  // which is what running a program repeatedly actually costs.
  const run = async (n) => {
    for (let i = 0; i < n; i++) {
      const wasi = new WASI(options);
      const imports = typeof wasi.getImportObject === "function"
        ? wasi.getImportObject()
        : { wasi_snapshot_preview1: wasi.wasiImport }; // Bun exposes it directly
      wasi.start(await WebAssembly.instantiate(module, imports));
    }
  };

  await run(RUNS / 10); // untimed warmup
  const t0 = performance.now();
  await run(RUNS);
  const t1 = performance.now();
  console.log("RESULT_MS=" + (t1 - t0).toFixed(2));
})();
