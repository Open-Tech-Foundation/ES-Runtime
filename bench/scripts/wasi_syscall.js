// WASI syscall benchmark: a guest whose `_start` loops 20 000 times calling
// `random_get` and `clock_time_get`, each writing its result into linear memory.
//
// The calls come from *inside* wasm, where a real program makes them, so this
// measures the preview-1 implementation on the host side — for esrun the pure-JS
// `runtime:wasi` module and the ops beneath it. Bootstrap is excluded by timing
// only `start()`, whose cost the wasi_start row measures separately.
(async () => {
  if (typeof WebAssembly === "undefined") return; // no wasm engine (n/a)
  const { wasiModule, loadWASI } = await import("./wasm-mod.js");
  const WASI = await loadWASI();
  if (!WASI) return; // runtime has no WASI (n/a)

  const SYSCALLS = 60_000;
  const options = { version: "preview1", args: ["prog"], env: {} };

  // A WASI instance may only be started once, so each `start()` needs its own —
  // prepared outside the timed region, which holds only the guest's syscalls.
  const prepare = async (syscalls) => {
    const wasi = new WASI(options);
    const imports = typeof wasi.getImportObject === "function"
      ? wasi.getImportObject()
      : { wasi_snapshot_preview1: wasi.wasiImport }; // Bun exposes it directly
    const module = await WebAssembly.compile(wasiModule({ syscalls }));
    return [wasi, await WebAssembly.instantiate(module, imports)];
  };

  const [warm, warmInstance] = await prepare(SYSCALLS / 10);
  warm.start(warmInstance); // untimed warmup
  const [wasi, instance] = await prepare(SYSCALLS);
  const t0 = performance.now();
  wasi.start(instance);
  const t1 = performance.now();
  console.log("RESULT_MS=" + (t1 - t0).toFixed(2));
})();
