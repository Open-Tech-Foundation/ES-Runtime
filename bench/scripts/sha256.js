// WebCrypto benchmark: SHA-256 of a 4 KiB buffer, many times. Measures the
// crypto backend (RustCrypto for esrun; BoringSSL/OpenSSL for Node/Deno/Bun)
// plus the per-call async/op overhead.
(async () => {
  const data = new Uint8Array(4096);
  crypto.getRandomValues(data);
  const N = 20_000;
  const run = async (n) => {
    let acc = 0;
    for (let i = 0; i < n; i++) {
      const d = await crypto.subtle.digest("SHA-256", data);
      acc ^= new Uint8Array(d)[0];
    }
    return acc;
  };
  await run(N / 10); // untimed JIT warmup
  const t0 = performance.now();
  const acc = await run(N);
  const t1 = performance.now();
  if (acc === -1) console.log(acc); // defeat dead-code elimination
  console.log("RESULT_MS=" + (t1 - t0).toFixed(2));
})();
