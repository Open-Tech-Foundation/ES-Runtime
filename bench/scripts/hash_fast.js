// Non-cryptographic hashing benchmark: a 64 KiB buffer through a checksum-grade
// hash, many times — the cache key, ETag or shard selector a server computes on
// a hot path, where collision resistance against an adversary is not the
// question being asked.
//
// Only two runtimes can answer without a package: esrun's `runtime:hashing`
// (XXH3) and `Bun.hash` (Wyhash). Node, Deno and LLRT have no non-cryptographic
// hash in their standard library, so the row is n/a for them — which is the
// finding, not a gap in the measurement. Compare against `hash_hex` to see what
// the choice is worth.
(async () => {
  const data = new Uint8Array(64 * 1024);
  crypto.getRandomValues(data);
  const N = 20_000;

  let hashOnce;
  let esrunHash = null;
  try {
    esrunHash = (await import("runtime:hashing")).hash;
  } catch {}

  if (typeof esrunHash === "function") {
    const out = esrunHash("xxhash3", data);
    if (out.length !== 8) throw new Error("unexpected xxhash3 width");
    hashOnce = () => esrunHash("xxhash3", data)[0];
  } else if (typeof Bun !== "undefined") {
    hashOnce = () => Number(Bun.hash(data) & 0xffn);
  } else {
    // No standard-library answer. Exiting non-zero is how the harness records
    // "this runtime cannot do this", distinct from a timeout.
    throw new Error("no non-cryptographic hash in this runtime's standard library");
  }

  const run = (n) => {
    let acc = 0;
    for (let i = 0; i < n; i++) acc ^= hashOnce();
    return acc;
  };

  run(N / 10); // untimed JIT warmup
  const t0 = performance.now();
  const acc = run(N);
  const t1 = performance.now();
  if (acc === -1) console.log(acc); // defeat dead-code elimination
  console.log("RESULT_MS=" + (t1 - t0).toFixed(2));
})();
