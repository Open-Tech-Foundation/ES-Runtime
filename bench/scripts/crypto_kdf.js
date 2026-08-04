// Key-derivation benchmark: PBKDF2-HMAC-SHA-256 at a fixed iteration count.
// A KDF is deliberately slow, so the runtime's per-call overhead is negligible
// and this measures the backend's inner hash loop almost purely — the one place
// a server's CPU goes on every login. The iteration count is well below a
// production setting so the row stays a few hundred milliseconds; it is a
// comparison between runtimes, not a security recommendation.
(async () => {
  const ITERATIONS = 10_000;
  const enc = new TextEncoder();
  const material = await crypto.subtle.importKey(
    "raw",
    enc.encode("correct horse battery staple"),
    "PBKDF2",
    false,
    ["deriveBits"],
  );
  const salt = enc.encode("a-fixed-salt-value");

  const N = 20;
  const run = async (n) => {
    let acc = 0;
    for (let i = 0; i < n; i++) {
      const bits = await crypto.subtle.deriveBits(
        { name: "PBKDF2", hash: "SHA-256", salt, iterations: ITERATIONS },
        material,
        256,
      );
      acc ^= new Uint8Array(bits)[0];
    }
    return acc;
  };

  await run(Math.max(N / 10, 2)); // untimed warmup
  const t0 = performance.now();
  const acc = await run(N);
  const t1 = performance.now();
  if (acc === -1) console.log(acc); // defeat dead-code elimination
  console.log("RESULT_MS=" + (t1 - t0).toFixed(2));
})();
