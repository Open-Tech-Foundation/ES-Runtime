// Asymmetric crypto benchmark: ECDSA P-256 sign + verify over a small payload,
// the shape a server hits issuing and checking a signed token on each request.
// `crypto` and `sha256` cover the symmetric and digest paths; public-key work is
// a different backend entirely (and, on most runtimes, a different library), so
// it ranks differently.
(async () => {
  const data = new TextEncoder().encode(
    JSON.stringify({ sub: "user-1234", exp: 1893456000, scope: "read write" }),
  );
  const alg = { name: "ECDSA", hash: "SHA-256" };
  const key = await crypto.subtle.generateKey(
    { name: "ECDSA", namedCurve: "P-256" },
    false,
    ["sign", "verify"],
  );

  const N = 2_000;
  const run = async (n) => {
    let acc = 0;
    for (let i = 0; i < n; i++) {
      const sig = await crypto.subtle.sign(alg, key.privateKey, data);
      if (await crypto.subtle.verify(alg, key.publicKey, sig, data)) acc++;
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
