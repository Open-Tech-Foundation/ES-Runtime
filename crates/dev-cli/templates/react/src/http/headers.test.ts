import { nonce, securityHeaders } from "./headers.ts";

test("a nonce is unguessable and never repeats", () => {
  const seen = new Set<string>();
  for (let i = 0; i < 1000; i++) {
    seen.add(nonce());
  }
  assertEquals(seen.size, 1000, "a nonce repeated");
  // 16 bytes, base64. A shorter one is a policy an attacker can brute-force.
  assert(nonce().length >= 22, nonce());
});

test("in production the policy admits the one inline script and no other", () => {
  const value = securityHeaders("t3stN0nc3", false)["content-security-policy"]!;
  assert(value.includes("script-src 'self' 'nonce-t3stN0nc3'"), value);
  // The whole point: with 'unsafe-inline' the nonce would be decorative,
  // because every injected script would be allowed too.
  assert(!value.includes("unsafe-inline"), value);
  assert(!value.includes("unsafe-eval"), value);
});

test("in development it also admits the local reload endpoint", () => {
  // `esdev start` serves live reload from another port on loopback, and the
  // page is served from this one — so `connect-src 'self'` blocks it and the
  // page silently stops reloading.
  const value = securityHeaders("n", true)["content-security-policy"]!;
  assert(value.includes("connect-src 'self' http://127.0.0.1:*"), value);
  // …and production stays closed.
  const production = securityHeaders("n", false)["content-security-policy"]!;
  assert(production.includes("connect-src 'self';"), production);
  assert(!production.includes("127.0.0.1"), production);
});

test("in development it allows inline script and drops the nonce", () => {
  // `esdev start` injects its live-reload client inline and it cannot carry a
  // nonce, so the production policy would block it and reloading would silently
  // stop working.
  const value = securityHeaders("t3stN0nc3", true)["content-security-policy"]!;
  assert(value.includes("script-src 'self' 'unsafe-inline'"), value);
  // Dropped, not merely accompanied: a policy carrying a nonce ignores
  // 'unsafe-inline' outright, so keeping both would block it exactly as before.
  assert(!value.includes("nonce-"), value);
  assert(!value.includes("unsafe-eval"), value);
});

test("the policy closes the openings that do not need a script at all", () => {
  const value = securityHeaders("n", false)["content-security-policy"]!;
  for (const directive of [
    "default-src 'self'",
    // Clickjacking.
    "frame-ancestors 'none'",
    // An injected <base href> silently re-points every relative URL on the page.
    "base-uri 'none'",
    // A form posting to another origin is a phishing page wearing this address.
    "form-action 'self'",
    "object-src 'none'",
  ]) {
    assert(value.includes(directive), `${directive} is missing from: ${value}`);
  }
});

test("every response carries the headers that cost nothing", () => {
  const headers = securityHeaders("n", false);
  assertEquals(headers["x-content-type-options"], "nosniff");
  assertEquals(headers["referrer-policy"], "strict-origin-when-cross-origin");
  assert(headers["permissions-policy"]!.includes("camera=()"));
});
