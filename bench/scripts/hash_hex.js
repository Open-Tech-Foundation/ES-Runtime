// Hashing benchmark: SHA-256 of a 4 KiB buffer to a **hex string**, many times.
//
// The pairing for `crypto|sha256`, which measures the same digest through
// `crypto.subtle` — async, and returning an ArrayBuffer a caller then has to
// encode themselves. This row measures what that caller actually wanted: a
// synchronous call whose result is the hex string, which is the shape every
// runtime except a bare WebCrypto one provides.
//
// Hashing is not a shared Web API in this shape, so each runtime uses its own:
// esrun's `runtime:hashing` `hash()`, `Bun.CryptoHasher`, and `node:crypto`
// `createHash` for Node, Deno and LLRT.
(async () => {
  const data = new Uint8Array(4096);
  crypto.getRandomValues(data);
  const N = 20_000;

  let hashOnce;
  let esrunHash = null;
  try {
    esrunHash = (await import("runtime:hashing")).hash;
  } catch {}

  if (typeof esrunHash === "function") {
    hashOnce = () => esrunHash("sha256", data, "hex");
  } else if (typeof Bun !== "undefined") {
    hashOnce = () => new Bun.CryptoHasher("sha256").update(data).digest("hex");
  } else {
    const { createHash } = await import("node:crypto");
    hashOnce = () => createHash("sha256").update(data).digest("hex");
  }

  const run = (n) => {
    let acc = 0;
    for (let i = 0; i < n; i++) acc ^= hashOnce().charCodeAt(0);
    return acc;
  };

  run(N / 10); // untimed JIT warmup
  const t0 = performance.now();
  const acc = run(N);
  const t1 = performance.now();
  if (acc === -1) console.log(acc); // defeat dead-code elimination
  console.log("RESULT_MS=" + (t1 - t0).toFixed(2));
})();
