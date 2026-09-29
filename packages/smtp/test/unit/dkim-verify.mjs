// A DKIM verifier (RFC 6376 §6), for the tests: it checks every
// DKIM-Signature on a message against public keys given by selector, the way a
// receiver checks them against DNS. Its canonicalization is the signer's own,
// which dkim.mjs proves against RFC 8463's published signatures.

import { canonicalBody, canonicalHeader, selectHeaders, split } from "../../dist/dkim.js";

const binary = (bytes) => String.fromCharCode(...bytes);
const octets = (text) => Uint8Array.from(text, (c) => c.charCodeAt(0));
const b64 = (text) => octets(atob(text));
const sha256 = async (data) => new Uint8Array(await crypto.subtle.digest("SHA-256", data));

/** `p=` of a DKIM TXT record, as a key: SPKI for RSA, the raw 32 bytes for Ed25519. */
export function publicKey(algorithm, p) {
  return algorithm === "rsa"
    ? crypto.subtle.importKey(
        "spki",
        b64(p),
        { name: "RSASSA-PKCS1-v1_5", hash: "SHA-256" },
        false,
        ["verify"],
      )
    : crypto.subtle.importKey("raw", b64(p), { name: "Ed25519" }, false, ["verify"]);
}

/**
 * `{ selector, algorithm, pass, reason }` for each signature, top down.
 * `keys` maps a selector to its public key.
 */
export async function verify(message, keys) {
  const text =
    typeof message === "string" ? binary(new TextEncoder().encode(message)) : binary(message);
  const { headers, body } = split(text.replace(/\r\n|\r|\n/g, "\r\n"));
  const results = [];
  for (const field of headers.filter((f) => f.name === "dkim-signature")) {
    const tags = Object.fromEntries(
      canonicalHeader(field.raw)
        .slice("dkim-signature:".length)
        .split(";")
        .map((t) => t.trim())
        .filter((t) => t !== "")
        .map((t) => [
          t.slice(0, t.indexOf("=")).trim(),
          t.slice(t.indexOf("=") + 1).replace(/\s+/g, ""),
        ]),
    );
    const result = { selector: tags.s, algorithm: tags.a, domain: tags.d, h: tags.h, pass: false };
    results.push(result);
    const bh = btoa(binary(await sha256(octets(canonicalBody(body)))));
    if (bh !== tags.bh) {
      result.reason = "body hash";
      continue;
    }
    const names = tags.h.split(":").map((n) => n.trim().toLowerCase());
    const unsigned = field.raw.replace(/(^|;)([ \t\r\n]*b[ \t\r\n]*=)[^;]*/, "$1$2");
    const data =
      selectHeaders(headers, names)
        .map((f) => `${canonicalHeader(f.raw)}\r\n`)
        .join("") + canonicalHeader(unsigned);
    const bytes = octets(data);
    const key = keys[tags.s];
    result.pass =
      tags.a === "rsa-sha256"
        ? await crypto.subtle.verify("RSASSA-PKCS1-v1_5", key, b64(tags.b), bytes)
        : await crypto.subtle.verify("Ed25519", key, b64(tags.b), await sha256(bytes));
    if (!result.pass) result.reason = "signature";
  }
  return results;
}
