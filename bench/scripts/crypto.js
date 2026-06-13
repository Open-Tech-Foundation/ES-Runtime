// WebCrypto breadth benchmark: per iteration one HMAC-SHA-256 sign and one
// AES-256-GCM encrypt+decrypt of a 1 KiB buffer, with a fresh random IV.
// Complements sha256 (which isolates one digest shape) by spreading across the
// key-based subtle surface plus getRandomValues.
(async () => {
  const data = new Uint8Array(1024);
  crypto.getRandomValues(data);
  const iv = new Uint8Array(12);
  const hmacKey = await crypto.subtle.importKey(
    "raw",
    new Uint8Array(32),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"],
  );
  const aesKey = await crypto.subtle.generateKey(
    { name: "AES-GCM", length: 256 },
    false,
    ["encrypt", "decrypt"],
  );
  const N = 2_000;
  const run = async (n) => {
    let acc = 0;
    for (let i = 0; i < n; i++) {
      crypto.getRandomValues(iv);
      const sig = await crypto.subtle.sign("HMAC", hmacKey, data);
      const ct = await crypto.subtle.encrypt({ name: "AES-GCM", iv }, aesKey, data);
      const pt = await crypto.subtle.decrypt({ name: "AES-GCM", iv }, aesKey, ct);
      acc += new Uint8Array(sig)[0] ^ new Uint8Array(pt)[0];
    }
    return acc;
  };
  await run(N / 10); // untimed warmup
  const t0 = performance.now();
  const acc = await run(N);
  const t1 = performance.now();
  if (acc === -1) console.log(acc); // defeat dead-code elimination
  console.log("RESULT_MS=" + (t1 - t0).toFixed(2));
})();
