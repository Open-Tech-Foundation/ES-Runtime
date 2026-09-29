// DKIM signing. The anchor is RFC 8463 Appendix A: its message, its two keys
// and the two signatures it publishes. RSASSA-PKCS1-v1_5 and Ed25519 are both
// deterministic, so the same key over the same canonical data must give the
// same bytes — which checks canonicalization, header selection, both key
// formats (PKCS#1 and PKCS#8) and the Ed25519-over-SHA-256 rule at once.

import {
  canonicalBody,
  canonicalHeader,
  importKey,
  prepareSigner,
  selectHeaders,
  sign,
  signData,
  split,
} from "../../dist/dkim.js";
import { buildMessage } from "../../dist/mime/message.js";
import { is, ok, report } from "./assert.mjs";
import { publicKey, verify } from "./dkim-verify.mjs";

const RSA_PKCS1 = `-----BEGIN RSA PRIVATE KEY-----
MIICXQIBAAKBgQDkHlOQoBTzWRiGs5V6NpP3idY6Wk08a5qhdR6wy5bdOKb2jLQi
Y/J16JYi0Qvx/byYzCNb3W91y3FutACDfzwQ/BC/e/8uBsCR+yz1Lxj+PL6lHvqM
KrM3rG4hstT5QjvHO9PzoxZyVYLzBfO2EeC3Ip3G+2kryOTIKT+l/K4w3QIDAQAB
AoGAH0cxOhFZDgzXWhDhnAJDw5s4roOXN4OhjiXa8W7Y3rhX3FJqmJSPuC8N9vQm
6SVbaLAE4SG5mLMueHlh4KXffEpuLEiNp9Ss3O4YfLiQpbRqE7Tm5SxKjvvQoZZe
zHorimOaChRL2it47iuWxzxSiRMv4c+j70GiWdxXnxe4UoECQQDzJB/0U58W7RZy
6enGVj2kWF732CoWFZWzi1FicudrBFoy63QwcowpoCazKtvZGMNlPWnC7x/6o8Gc
uSe0ga2xAkEA8C7PipPm1/1fTRQvj1o/dDmZp243044ZNyxjg+/OPN0oWCbXIGxy
WvmZbXriOWoSALJTjExEgraHEgnXssuk7QJBALl5ICsYMu6hMxO73gnfNayNgPxd
WFV6Z7ULnKyV7HSVYF0hgYOHjeYe9gaMtiJYoo0zGN+L3AAtNP9huqkWlzECQE1a
licIeVlo1e+qJ6Mgqr0Q7Aa7falZ448ccbSFYEPD6oFxiOl9Y9se9iYHZKKfIcst
o7DUw1/hz2Ck4N5JrgUCQQCyKveNvjzkkd8HjYs0SwM0fPjK16//5qDZ2UiDGnOe
uEzxBDAr518Z8VFbR41in3W4Y3yCDgQlLlcETrS+zYcL
-----END RSA PRIVATE KEY-----`;
const RSA_PUBLIC =
  "MIGfMA0GCSqGSIb3DQEBAQUAA4GNADCBiQKBgQDkHlOQoBTzWRiGs5V6NpP3idY6Wk08a5qhdR6wy5bdOKb2jLQiY/J16JYi0Qvx/byYzCNb3W91y3FutACDfzwQ/BC/e/8uBsCR+yz1Lxj+PL6lHvqMKrM3rG4hstT5QjvHO9PzoxZyVYLzBfO2EeC3Ip3G+2kryOTIKT+l/K4w3QIDAQAB";

// RFC 8463 gives the Ed25519 secret as its raw 32-byte seed; a key file holds
// it in PKCS#8, which is that seed behind a fixed 16-byte prefix.
const ED25519_SEED = "nWGxne/9WmC6hEr0kuwsxERJxWl7MmkZcDusAxyuf2A=";
const ED25519_PKCS8 = `-----BEGIN PRIVATE KEY-----\n${btoa(
  String.fromCharCode(
    ...[
      0x30, 0x2e, 0x02, 0x01, 0x00, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x04, 0x22, 0x04,
      0x20,
    ],
    ...Uint8Array.from(atob(ED25519_SEED), (c) => c.charCodeAt(0)),
  ),
)}\n-----END PRIVATE KEY-----`;
const ED25519_PUBLIC = "11qYAYKxCrfVS/7TyWQHOg7hcvPapiMlrwIaaPcHURo=";

const RFC_MESSAGE = [
  "DKIM-Signature: v=1; a=ed25519-sha256; c=relaxed/relaxed;",
  " d=football.example.com; i=@football.example.com;",
  " q=dns/txt; s=brisbane; t=1528637909; h=from : to :",
  " subject : date : message-id : from : subject : date;",
  " bh=2jUSOH9NhtVGCQWNr9BrIAPreKQjO6Sn7XIkfJVOzv8=;",
  " b=/gCrinpcQOoIfuHNQIbq4pgh9kyIK3AQUdt9OdqQehSwhEIug4D11Bus",
  " Fa3bT3FY5OsU7ZbnKELq+eXdp1Q1Dw==",
  "DKIM-Signature: v=1; a=rsa-sha256; c=relaxed/relaxed;",
  " d=football.example.com; i=@football.example.com;",
  " q=dns/txt; s=test; t=1528637909; h=from : to : subject :",
  " date : message-id : from : subject : date;",
  " bh=2jUSOH9NhtVGCQWNr9BrIAPreKQjO6Sn7XIkfJVOzv8=;",
  " b=F45dVWDfMbQDGHJFlXUNB2HKfbCeLRyhDXgFpEL8GwpsRe0IeIixNTe3",
  " DhCVlUrSjV4BwcVcOF6+FF3Zo9Rpo1tFOeS9mPYQTnGdaSGsgeefOsk2Jz",
  " dA+L10TeYt9BgDfQNZtKdN1WO//KgIqXP7OdEFE4LjFYNcUxZQ4FADY+8=",
  "From: Joe SixPack <joe@football.example.com>",
  "To: Suzie Q <suzie@shopping.example.net>",
  "Subject: Is dinner ready?",
  "Date: Fri, 11 Jul 2003 21:00:37 -0700 (PDT)",
  "Message-ID: <20030712040037.46341.5F8J@football.example.com>",
  "",
  "Hi.",
  "",
  "We lost the game.  Are you hungry yet?",
  "",
  "Joe.",
  "",
].join("\r\n");

const keys = {
  brisbane: await publicKey("ed25519", ED25519_PUBLIC),
  test: await publicKey("rsa", RSA_PUBLIC),
};
const rsa = await importKey(RSA_PKCS1);
const ed25519 = await importKey(ED25519_PKCS8);
is(rsa.algorithm, "rsa-sha256", "a PKCS#1 PEM imports as an RSA key");
is(ed25519.algorithm, "ed25519-sha256", "a PKCS#8 PEM names its algorithm");

// --- RFC 8463, byte for byte -----------------------------------------------------------

{
  const { headers, body } = split(RFC_MESSAGE);
  const bh = btoa(
    String.fromCharCode(
      ...new Uint8Array(
        await crypto.subtle.digest(
          "SHA-256",
          Uint8Array.from(canonicalBody(body), (c) => c.charCodeAt(0)),
        ),
      ),
    ),
  );
  is(bh, "2jUSOH9NhtVGCQWNr9BrIAPreKQjO6Sn7XIkfJVOzv8=", "the body hash is RFC 8463's");

  const names = "from:to:subject:date:message-id:from:subject:date".split(":");
  for (const [index, key, what] of [
    [0, ed25519, "Ed25519"],
    [1, rsa, "RSA"],
  ]) {
    const field = headers.filter((f) => f.name === "dkim-signature")[index];
    const published = /b=([^;]*)$/.exec(field.raw)[1].replace(/\s+/g, "");
    const unsigned = field.raw.replace(/b=[^;]*$/, "b=");
    const data =
      selectHeaders(headers, names)
        .map((f) => `${canonicalHeader(f.raw)}\r\n`)
        .join("") + canonicalHeader(unsigned);
    const b = await signData(
      key,
      Uint8Array.from(data, (c) => c.charCodeAt(0)),
    );
    is(b, published, `the ${what} signature is RFC 8463's, byte for byte`);
  }

  const results = await verify(RFC_MESSAGE, keys);
  ok(
    results.length === 2 && results.every((r) => r.pass),
    "the test verifier passes both of the RFC's signatures",
  );
}

// --- canonicalization edges ------------------------------------------------------------

is(canonicalHeader("SubJect :  a \t b  \r\n\t c  "), "subject:a b c", "a header, relaxed");
is(canonicalBody(" a  \t b \r\n\r\n\r\n"), " a b\r\n", "trailing whitespace and empty lines go");
is(canonicalBody(""), "", "an empty body is empty");
is(canonicalBody("\r\n\r\n"), "", "a body of empty lines is empty");
is(canonicalBody("no break"), "no break\r\n", "a last line gets its CRLF");
{
  const { headers, body } = split("A: 1\r\nB: 2\r\n  more\r\nA: 3\r\n\r\nbody");
  is(
    headers.map((h) => h.name),
    ["a", "b", "a"],
    "fields split, folds joined",
  );
  is(body, "body", "the body follows the empty line");
  is(
    selectHeaders(headers, ["a", "a", "a", "b"]).map((f) => f.raw),
    ["A: 3", "A: 1", "B: 2\r\n  more"],
    "a repeated name takes the next instance up; past the last it takes none",
  );
}

// --- a message signed here verifies ------------------------------------------------------

const signers = [
  await prepareSigner({
    domain: "Football.Example.com",
    selector: "brisbane",
    privateKey: ED25519_PKCS8,
  }),
  await prepareSigner({ domain: "football.example.com", selector: "test", privateKey: RSA_PKCS1 }),
];
const built = await buildMessage({
  from: "Joe <joe@football.example.com>",
  to: "suzie@shopping.example.net",
  cc: "coach@football.example.com",
  subject: "Grüße — dinner?",
  text: "Hi.\n.\nA line that is a dot, trailing space   \n\n\n",
  html: "<p>Hi.</p>",
  headers: { "List-Unsubscribe": "<https://example.com/u/1>", "X-Campaign": "7" },
});
const signed = new TextDecoder().decode(await sign(built.text, signers, new Date(1528637909000)));
{
  const results = await verify(signed, keys);
  is(
    results.map((r) => r.pass),
    [true, true],
    "both signatures verify",
  );
  is(results[0].domain, "football.example.com", "d= is lowercased");
  ok(signed.includes("t=1528637909;"), "t= is the signing time");
  const h = results[1].h.split(":");
  is(h.filter((n) => n === "from").length, 2, "From is oversigned");
  is(h.filter((n) => n === "cc").length, 2, "Cc is oversigned");
  ok(h.includes("list-unsubscribe"), "List-Unsubscribe is signed (RFC 8058)");
  ok(!h.includes("x-campaign"), "a header not on the list is not");
  ok(!/\bl=/.test(signed.slice(0, signed.indexOf("\r\nDate:"))), "no l= is ever written");
  ok(
    signed.split("\r\n").every((line) => line.length <= 998),
    "every line of the signed message fits SMTP",
  );
  ok(signed.startsWith("DKIM-Signature: v=1; a=ed25519-sha256;"), "the signatures come first");
}

// What a relay does leaves it valid; what a forger does breaks it.
{
  const pass = async (text) => (await verify(text, keys)).map((r) => r.pass);
  is(
    await pass(`Received: from relay by mx; Fri, 1 Jan 2027 00:00:00 +0000\r\n${signed}`),
    [true, true],
    "a Received header added in transit",
  );
  is(
    await pass(
      signed.replace("\r\nSubject: ", "\r\nSubject:\r\n\t  ").replace(/\r\n$/, "  \r\n\r\n"),
    ),
    [true, true],
    "a refolded header and trailing whitespace",
  );
  is(
    await pass(signed.replace("\r\nFrom: ", "\r\nFrom: Mallory <m@evil.example>\r\nFrom: ")),
    [false, false],
    "a second From",
  );
  is(
    await pass(signed.replace("\r\nTo: ", "\r\nCc: extra@evil.example\r\nTo: ")),
    [false, false],
    "an added Cc",
  );
  is(await pass(signed.replace("Hi.", "Hi!")), [false, false], "a changed body");
  is(await pass(`${signed}P.S.\r\n`), [false, false], "an appended line");
}

// Bare LF and 8-bit octets in a raw message are signed as they are sent.
{
  const raw = new Uint8Array([
    ...new TextEncoder().encode("From: a@football.example.com\nSubject: raw\n\nnaïve "),
    0xff,
    ...new TextEncoder().encode("\n"),
  ]);
  const out = await sign(raw, signers);
  const results = await verify(out, keys);
  is(
    results.map((r) => r.pass),
    [true, true],
    "a raw message with bare LF and 8-bit octets",
  );
  ok(out.includes(0xff), "its octets are kept");
}

// --- refusals ----------------------------------------------------------------------------

const refuses = async (fn, what) => {
  try {
    await fn();
    ok(false, `${what} — nothing was thrown`);
  } catch (e) {
    is(e.code, "ERR_SMTP_DKIM", what);
  }
};
await refuses(() => sign("Subject: x\r\n\r\nbody", signers), "a message without From");
await refuses(
  () => prepareSigner({ domain: "exa mple.com", selector: "s", privateKey: RSA_PKCS1 }),
  "a domain with a space",
);
await refuses(
  () => prepareSigner({ domain: "bücher.example", selector: "s", privateKey: RSA_PKCS1 }),
  "a domain not in its xn-- form",
);
await refuses(
  () => prepareSigner({ domain: "example.com", selector: "s;x=1", privateKey: RSA_PKCS1 }),
  "a selector that would add a tag",
);
await refuses(() => importKey("not a key"), "a string that is not PEM");
await refuses(
  () =>
    importKey("-----BEGIN ENCRYPTED PRIVATE KEY-----\nAAAA\n-----END ENCRYPTED PRIVATE KEY-----"),
  "an encrypted key",
);
await refuses(
  () => importKey("-----BEGIN EC PRIVATE KEY-----\nAAAA\n-----END EC PRIVATE KEY-----"),
  "an EC key, which DKIM has no algorithm for",
);
{
  const short = await crypto.subtle.generateKey(
    {
      name: "RSASSA-PKCS1-v1_5",
      modulusLength: 512,
      publicExponent: new Uint8Array([1, 0, 1]),
      hash: "SHA-256",
    },
    false,
    ["sign", "verify"],
  );
  await refuses(() => importKey(short.privateKey), "an RSA key under 1024 bits");
  await refuses(() => importKey(short.publicKey), "a public key");
  const hmac = await crypto.subtle.generateKey({ name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
  await refuses(() => importKey(hmac), "an HMAC key");
  const sha1 = await crypto.subtle.generateKey(
    {
      name: "RSASSA-PKCS1-v1_5",
      modulusLength: 2048,
      publicExponent: new Uint8Array([1, 0, 1]),
      hash: "SHA-1",
    },
    false,
    ["sign"],
  );
  await refuses(() => importKey(sha1.privateKey), "an RSA key for SHA-1");
  const good = await crypto.subtle.generateKey({ name: "Ed25519" }, false, ["sign", "verify"]);
  is(
    (await importKey(good.privateKey)).algorithm,
    "ed25519-sha256",
    "a CryptoKey is used as it is",
  );
}
if (report("dkim") > 0) (await import("runtime:process")).exit(1);
