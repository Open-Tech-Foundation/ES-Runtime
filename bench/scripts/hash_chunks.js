// Incremental hashing benchmark: SHA-256 over 4 MiB fed in 64 KiB chunks — the
// shape a file or an upload actually arrives in.
//
// This is the case `crypto.subtle.digest` cannot express at all: its signature
// takes the whole input, so the same work there means holding all 4 MiB. The
// row therefore measures the update path — per-chunk call overhead on top of
// the compression function — rather than one large digest.
//
// Each runtime uses its own incremental hasher: esrun's `runtime:hashing`
// `Hasher`, `Bun.CryptoHasher`, and `node:crypto` `createHash` for Node, Deno
// and LLRT.
(async () => {
  const CHUNK = 64 * 1024;
  const CHUNKS = 64; // 4 MiB per digest
  const N = 200;

  // One buffer, read repeatedly: this measures hashing, not allocation.
  const chunk = new Uint8Array(CHUNK);
  crypto.getRandomValues(chunk);

  let digestOnce;
  let Hasher = null;
  try {
    Hasher = (await import("runtime:hashing")).Hasher;
  } catch {}

  if (typeof Hasher === "function") {
    digestOnce = () => {
      const h = new Hasher("sha256");
      for (let i = 0; i < CHUNKS; i++) h.update(chunk);
      return h.digest();
    };
  } else if (typeof Bun !== "undefined") {
    digestOnce = () => {
      const h = new Bun.CryptoHasher("sha256");
      for (let i = 0; i < CHUNKS; i++) h.update(chunk);
      return h.digest();
    };
  } else {
    const { createHash } = await import("node:crypto");
    digestOnce = () => {
      const h = createHash("sha256");
      for (let i = 0; i < CHUNKS; i++) h.update(chunk);
      return h.digest();
    };
  }

  const run = (n) => {
    let acc = 0;
    for (let i = 0; i < n; i++) acc ^= digestOnce()[0];
    return acc;
  };

  run(Math.max(1, N / 10)); // untimed JIT warmup
  const t0 = performance.now();
  const acc = run(N);
  const t1 = performance.now();
  if (acc === -1) console.log(acc); // defeat dead-code elimination
  console.log("RESULT_MS=" + (t1 - t0).toFixed(2));
})();
