// WASM linear-memory benchmark: JS fills a 64 KiB window of the instance's
// memory through a typed-array view and wasm sums it back, 8 000 times. This is
// the shape most real wasm interop takes — bytes handed over a shared buffer
// rather than through arguments.
(async () => {
  if (typeof WebAssembly === "undefined") return; // no wasm engine (n/a)
  const { memoryModule } = await import("./wasm-mod.js");

  const CHUNK = 64 * 1024;
  const ROUNDS = 8_000;
  const { instance } = await WebAssembly.instantiate(memoryModule(4), {});
  const { memory, sum8 } = instance.exports;
  const view = new Uint8Array(memory.buffer);
  const src = new Uint8Array(CHUNK);
  for (let i = 0; i < CHUNK; i++) src[i] = i & 0x7f;

  const round = (n) => {
    let acc = 0;
    for (let i = 0; i < n; i++) {
      view.set(src, 0);
      acc = (acc + sum8(0, CHUNK)) | 0;
    }
    return acc;
  };

  round(ROUNDS / 10); // untimed JIT warmup
  const t0 = performance.now();
  const acc = round(ROUNDS);
  const t1 = performance.now();
  if (acc === -1) console.log(acc); // defeat dead-code elimination
  console.log("RESULT_MS=" + (t1 - t0).toFixed(2));
})();
