# Cross-runtime benchmark

Compares **esrun** (the ES-Runtime CLI) against **Node.js**, **Bun**, and
**Deno** on a handful of Web-API workloads. Every workload uses only APIs common
to all four runtimes, so the same script (`scripts/*.js`) runs unmodified on each.

## Running

```sh
cargo build --release -p es-runtime-cli   # build esrun first
bench/run.sh                              # auto-detects node / bun / deno / esrun
```

Knobs (env vars): `ESRUN=/path/to/esrun`, `STARTUP_RUNS` (default 25),
`WORKLOAD_RUNS` (default 3). A runtime that isn't installed is skipped.

## What each workload measures

| Workload | What it stresses |
| --- | --- |
| **startup** | Process launch + parse + teardown (near-empty script), measured as min process wall-time. |
| **compute** | A 20M-iteration numeric loop — mostly the JS engine (V8 for esrun/Node/Deno, JavaScriptCore for Bun). |
| **sha256** | 20 000 × SHA-256 of a 4 KiB buffer via `crypto.subtle.digest` — the crypto backend + per-call async overhead. |
| **webapi** | 100 000 × `new URL(...)` + `TextEncoder`/`TextDecoder` — for esrun this crosses the JS↔Rust op boundary per call; the others parse natively. |

## Representative results

Times in **milliseconds, lower is better**. One Linux x86-64 box; numbers are
indicative and will vary by machine — re-run locally for your own.

```
runtime  |    startup |    compute |     sha256 |     webapi
---------+------------+------------+------------+------------
node     |       18.5 |      212.7 |      697.1 |      131.6
bun      |        9.2 |      122.7 |      513.0 |      108.0
deno     |       24.3 |      226.1 |      600.9 |      207.3
esrun    |       15.3 |      250.0 |      367.0 |      410.0
```

(node v24, bun 1.3, deno 2.8, esrun 0.0.0.)

## Interpretation

- **startup ~15 ms** — esrun is competitive (faster than Node and Deno, behind
  Bun), despite being a single ~70 MB statically-linked binary. The baked-in V8
  startup snapshot (the whole prelude pre-executed) pays off here.
- **compute** — esrun lands in the V8 pack, a bit behind Node/Deno (same engine;
  the gap is build/wrapper overhead) and well behind Bun's JavaScriptCore on
  this particular loop.
- **sha256 — esrun is fastest.** `crypto.subtle.digest` in esrun is a synchronous
  RustCrypto op wrapped in an already-resolved promise, so 20 000 `await`s drain
  in microtask checkpoints with little scheduling cost; Node/Deno/Bun run a
  genuinely-async WebCrypto that pays per-call scheduling overhead. A real,
  explainable win **for this access pattern** (not a claim that RustCrypto beats
  BoringSSL raw).
- **webapi — esrun is slowest**, but the gap is in the prelude/op layer, not the
  engine. Decomposing `new URL()` (100k, with a query): the `url_parse` op +
  eager full-component JSON build + marshal is ~40%, `JSON.parse` + object
  construction ~20%, and the rest was eager `URLSearchParams` query parsing —
  now **lazy** (built only when `.searchParams` is read), which cut the URL
  workload ~38%. The remaining gap is the JSON round-trip (a structured-`Value`
  follow-up would remove it, D3a) plus esrun's pure-JS `TextEncoder`/`Decoder`
  vs the others' native ones. Op *dispatch* itself is cheap (~49 ns/call);
  the cost is per-call *work*, not the boundary crossing.

## Caveats

- These are **microbenchmarks** — they isolate one thing each and don't predict
  whole-application performance.
- esrun runs **single-file classic scripts** (no ES-module loader) and grants all
  capabilities — it's a convenience runner, not a sandbox here.
- The crypto and webapi shapes reflect esrun's **op model** (sync ops wrapped in
  promises; per-call boundary crossings) as much as the underlying libraries.
