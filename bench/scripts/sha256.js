// WebCrypto benchmark: SHA-256 of a 4 KiB buffer, many times. Measures the
// crypto backend (RustCrypto for esrun; BoringSSL/OpenSSL for Node/Deno/Bun)
// plus the per-call async/op overhead.
(async () => {
  const data = new Uint8Array(4096);
  crypto.getRandomValues(data);
  const N = 20_000;
  const t0 = performance.now();
  let last = 0;
  for (let i = 0; i < N; i++) {
    const d = await crypto.subtle.digest("SHA-256", data);
    last ^= new Uint8Array(d)[0];
  }
  const t1 = performance.now();
  if (last === -1) console.log(last);
  console.log("RESULT_MS=" + (t1 - t0).toFixed(2));
})();
