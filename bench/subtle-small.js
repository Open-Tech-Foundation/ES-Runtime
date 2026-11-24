// What moving crypto.subtle off the loop thread costs small calls (D241).
//
//   esrun bench/subtle-small.js
//
// Since D241 every crypto.subtle operation whose cost can grow runs on the
// blocking pool, so even a 32-byte HMAC pays a hop to a pool thread and back.
// Run it against a build from before D241 and one after: each row is the mean
// time a call takes one after another ("serial", latency) and the rate with 64
// calls in flight ("64 at once", throughput, where the pool runs them in
// parallel). Best of five.
const enc = new TextEncoder();
const small = enc.encode("x".repeat(32));
const kib = new Uint8Array(1024);

const hmacKey = await crypto.subtle.importKey(
  "raw",
  new Uint8Array(32),
  { name: "HMAC", hash: "SHA-256" },
  false,
  ["sign"],
);
const aesKey = await crypto.subtle.importKey("raw", new Uint8Array(16), "AES-GCM", false, ["encrypt"]);
const ec = await crypto.subtle.generateKey({ name: "ECDSA", namedCurve: "P-256" }, false, ["sign", "verify"]);
const ecSig = await crypto.subtle.sign({ name: "ECDSA", hash: "SHA-256" }, ec.privateKey, small);
const iv = new Uint8Array(12);

const rows = [
  ["digest SHA-256, 32 B", () => crypto.subtle.digest("SHA-256", small), 20000],
  ["HMAC-SHA256 sign, 32 B", () => crypto.subtle.sign("HMAC", hmacKey, small), 20000],
  ["AES-GCM encrypt, 1 KiB", () => crypto.subtle.encrypt({ name: "AES-GCM", iv }, aesKey, kib), 20000],
  ["ECDSA P-256 verify", () => crypto.subtle.verify({ name: "ECDSA", hash: "SHA-256" }, ec.publicKey, ecSig, small), 2000],
];

async function serial(call, n) {
  const start = performance.now();
  for (let i = 0; i < n; i++) await call();
  return ((performance.now() - start) * 1000) / n;
}

async function parallel(call, n) {
  const start = performance.now();
  let next = 0;
  await Promise.all(
    Array.from({ length: 64 }, async () => {
      while (next++ < n) await call();
    }),
  );
  return n / ((performance.now() - start) / 1000);
}

for (const [label, call, n] of rows) {
  for (let i = 0; i < 200; i++) await call();
  let us = Infinity;
  let rate = 0;
  for (let round = 0; round < 5; round++) {
    us = Math.min(us, await serial(call, n));
    rate = Math.max(rate, await parallel(call, n));
  }
  console.log(
    `${label.padEnd(26)} serial ${us.toFixed(2).padStart(7)} µs   64 at once ${(rate / 1000).toFixed(0).padStart(5)}k/s`,
  );
}
