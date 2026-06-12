// WebCrypto: random, digest, and an ECDSA sign/verify round-trip.
const id = crypto.randomUUID();
console.log("randomUUID:", id);

const data = new TextEncoder().encode("the quick brown fox");
const digest = await crypto.subtle.digest("SHA-256", data);
const hex = [...new Uint8Array(digest)]
  .map((b) => b.toString(16).padStart(2, "0"))
  .join("");
console.log("SHA-256:", hex);

const keyPair = await crypto.subtle.generateKey(
  { name: "ECDSA", namedCurve: "P-256" },
  true,
  ["sign", "verify"],
);
const signature = await crypto.subtle.sign(
  { name: "ECDSA", hash: "SHA-256" },
  keyPair.privateKey,
  data,
);
const ok = await crypto.subtle.verify(
  { name: "ECDSA", hash: "SHA-256" },
  keyPair.publicKey,
  signature,
  data,
);
console.log("ECDSA P-256 signature verified:", ok);
