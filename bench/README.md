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
`WORKLOAD_RUNS` (default 5). A runtime that isn't installed is skipped.

## What each workload measures

| Workload | What it stresses |
| --- | --- |
| **startup** | Process launch + parse + teardown (near-empty script), measured as min process wall-time. |
| **compute** | A 20M-iteration numeric loop — mostly the JS engine (V8 for esrun/Node/Deno, JavaScriptCore for Bun). |
| **json** | 200 000 × `JSON.stringify` + `JSON.parse` of a small object — pure engine work, no host crossings; a baseline separating engine speed from runtime-layer cost. |
| **sha256** | 20 000 × SHA-256 of a 4 KiB buffer via `crypto.subtle.digest` — the crypto backend + per-call async overhead. |
| **url** | 100 000 × `new URL(...)` + component reads — for esrun one JS↔Rust op per parse; the others parse natively. |
| **encoding** | 100 000 × `TextEncoder`/`TextDecoder` UTF-8 round trips — op-boundary crossings riding V8's native transcoding. |

Methodology: startup is the **min** over `STARTUP_RUNS` (the floor is the launch
cost); the other workloads run an untimed JIT warmup, time themselves with
`performance.now()`, and report the **median** of `WORKLOAD_RUNS` self-timed
runs, so a single noisy run can't set the number.

## Representative results

Times in **milliseconds, lower is better**. One Linux x86-64 box; numbers are
indicative and will vary by machine — re-run locally for your own.

```
runtime  |   startup |   compute |      json |    sha256 |       url |  encoding
---------+-----------+-----------+-----------+-----------+-----------+-----------
node     |      17.0 |     195.1 |     286.9 |     564.7 |      51.3 |      68.1
bun      |       8.7 |     120.8 |     210.9 |     427.1 |      77.0 |      22.8
esrun    |       8.5 |     232.6 |     193.2 |     336.6 |      83.5 |      68.8
```

(node v24, bun 1.3, esrun 0.0.0.)

## Interpretation

- **startup — esrun is fastest** (~8.5 ms, edging out Bun), despite being a
  single ~70 MB statically-linked binary. Two things pay for this: the baked-in
  V8 startup snapshot (the whole prelude pre-executed), and **lazy provider
  build-out** — the HTTP client (TLS config, root store) is constructed on the
  first `fetch`, not at boot, so scripts that never fetch never pay for it.
  Isolated on the same build, the eager client costs ~5.5 ms of the old ~15 ms
  startup; that change alone is most of the 15 → 8.5 ms drop.
- **compute** — esrun lands in the V8 pack but ~15% behind Node (same engine).
  Flag experiments (`--maglev`, `--max-opt`, etc.) move nothing — Maglev and
  concurrent compilation are already on. The residual gap is attributed to the
  prebuilt `rusty_v8` library's build configuration (e.g. pointer compression,
  which Node builds without) and V8 version skew — not addressable from this
  repo. Well behind Bun's JavaScriptCore on this particular loop.
- **json, sha256 — esrun is fastest.** JSON is pure engine work, so this is a
  sanity check that the engine itself is not the bottleneck elsewhere. For
  sha256, `crypto.subtle.digest` in esrun is a synchronous RustCrypto op wrapped
  in an already-resolved promise, so 20 000 `await`s drain in microtask
  checkpoints with little scheduling cost; Node/Deno/Bun run a genuinely-async
  WebCrypto that pays per-call scheduling overhead. A real, explainable win
  **for this access pattern** (not a claim that RustCrypto beats BoringSSL raw).
- **url, encoding — competitive** (url: behind Node's native Ada parser, near
  Bun; encoding: level with Node, behind Bun). This surface crosses the JS↔Rust
  op boundary on every call, and got here through three rounds of work worth
  recording:
  1. **Op dispatch itself is cheap** (~49 ns/call); the cost was always the
     per-call *work*, never the boundary crossing.
  2. **Structured marshaling was tried and reverted.** Returning URL components
     as a built JS object (~30 V8 calls per URL) was *slower* than serializing
     JSON in Rust and `JSON.parse`-ing it in JS (one optimized C++ pass).
  3. **Offsets beat both.** `url_parse` now returns the canonical href plus 15
     component *offsets* (the `url` crate's `Position`s, as one small JS array);
     every URL getter is a lazy `href.slice(...)`. Nothing is parsed, built, or
     allocated for components a script never reads (`URLSearchParams` included).
     This is the same shape Node's Ada integration uses, and it replaced the
     JSON round-trip wholesale (~3× on the URL workload, on top of the earlier
     ~38% from lazy `searchParams`).

  Encoding took the complementary fix: op results are **consumed, not copied** —
  a returned byte buffer now *moves* into the `ArrayBuffer` backing store
  (previously: two extra copies per `encode()`), and `decode()` converts valid
  UTF-8 in place. The remaining gap to Bun is JavaScriptCore's specialized
  encoder fast paths.

## Caveats

- These are **microbenchmarks** — they isolate one thing each and don't predict
  whole-application performance.
- esrun runs **single-file classic scripts** (no ES-module loader) and grants all
  capabilities — it's a convenience runner, not a sandbox here.
- The crypto shape reflects esrun's **op model** (sync ops wrapped in promises)
  as much as the underlying libraries.
- Not yet covered: `fetch` against a local server (the workload that would
  actually exercise the provider seam), `atob`/`btoa`, `getRandomValues`, and
  timer churn.
