// Nanoseconds per call for Web APIs whose cost is mostly the crossing between
// JS and the host, not the work: a clock read, a parse, a small transcode.
// The same file runs unmodified on esrun, Node, Deno and Bun:
//
//   esrun bench/op-cost.js; node bench/op-cost.js; deno run bench/op-cost.js; bun bench/op-cost.js
//
// Each row runs a tenth of its count untimed first, so the JIT has settled.
// Single runs on a quiet machine; repeat a row that looks off.
function bench(name, n, fn) {
  for (let i = 0; i < n / 10; i++) fn(i);
  const t0 = performance.now();
  for (let i = 0; i < n; i++) fn(i);
  const ns = ((performance.now() - t0) * 1e6) / n;
  console.log(name.padEnd(22), ns.toFixed(0).padStart(6), "ns/op");
}

const s1k = "x".repeat(1024);
const b1k = btoa(s1k);
const enc = new TextEncoder();
const dec = new TextDecoder();
const small = enc.encode("hello world");
const obj = { a: 1, b: "two", c: [1, 2, 3], d: { e: true } };

bench("performance.now", 1e6, () => performance.now());
bench("new URL", 2e5, (i) => new URL("https://example.com/p/" + (i & 7) + "?q=1"));
bench("url.pathname get", 1e6, (() => { const u = new URL("https://a.b/c?d"); return () => u.pathname; })());
bench("new Headers(3)", 2e5, () => new Headers({ a: "1", "content-type": "text/plain", x: "y" }));
bench("headers.get", 1e6, (() => { const h = new Headers({ a: "1", "content-type": "t" }); return () => h.get("content-type"); })());
bench("btoa 1KiB", 2e5, () => btoa(s1k));
bench("atob 1KiB", 2e5, () => atob(b1k));
bench("encode 11B", 1e6, () => enc.encode("hello world"));
bench("decode 11B", 1e6, () => dec.decode(small));
bench("structuredClone", 2e5, () => structuredClone(obj));
bench("crypto.randomUUID", 5e5, () => crypto.randomUUID());
bench("new Response(str)", 2e5, () => new Response("hi"));
bench("new Request(url)", 2e5, () => new Request("https://a.b/c"));
