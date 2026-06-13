# Cross-runtime benchmark

Compares **esrun** (the ES-Runtime CLI) against **Node.js**, **Bun**, and
**Deno** on a spread of Web-API workloads. Every workload uses only APIs common
to all four runtimes, so the same script (`scripts/*.js`) runs unmodified on each.

## Running

```sh
cargo build --release -p es-runtime-cli   # build esrun first
bench/run.sh                              # auto-detects node / bun / deno / esrun
```

Knobs (env vars): `ESRUN=/path/to/esrun`, `STARTUP_RUNS` (default 25),
`WORKLOAD_RUNS` (default 5), `WORKLOADS="url encoding"` (run a subset),
`BENCH_JSON=1` (machine-readable output for diffing runs over time). A runtime
that isn't installed is skipped; Deno is also looked for at `~/.deno/bin/deno`
and `/tmp/deno/bin/deno` if not on `PATH`.

## What each workload measures

| Workload | What it stresses |
| --- | --- |
| **startup** | Process launch + parse + teardown (near-empty script); min process wall-time. |
| **bigscript** | Same, on a generated ~100 KB script — isolates user-source **parse** cost (the snapshot pre-bakes only the prelude). |
| **compute** | 20M-iteration numeric loop — mostly the JS engine (V8 for esrun/Node/Deno, JavaScriptCore for Bun). |
| **json** | 200 000 × stringify+parse of a small object — pure engine, no host crossings; a baseline. |
| **jsonbig** | parse+stringify of one ~5 MB document — allocation/GC throughput rather than per-call overhead. |
| **sha256** | 20 000 × SHA-256 of a 4 KiB buffer via `crypto.subtle.digest` — crypto backend + per-call async overhead. |
| **crypto** | 2 000 × (HMAC-SHA-256 sign + AES-256-GCM encrypt/decrypt of 1 KiB, fresh IV) — the key-based `subtle` surface + `getRandomValues`. |
| **url** | 100 000 × `new URL(...)` + component reads — for esrun one JS↔Rust op per parse; the others parse natively. |
| **encoding** | 100 000 × `TextEncoder`/`TextDecoder` UTF-8 round trips — op crossings riding V8's native transcoding. |
| **base64** | 10 000 × `btoa`/`atob` of a 1 KiB string — op-backed for esrun; native elsewhere. |
| **structured** | 50 000 × `structuredClone` of a nested object — pure-JS recursive clone for esrun. |
| **async** | 1 000 000 × `await Promise.resolve(...)` — the microtask machinery and (for esrun) the driven loop's checkpoint. |
| **timers** | 10 000 zero-delay `setTimeout`s drained to completion — timer scheduling + driver. |
| **streams** | `ReadableStream`→`TransformStream`→`WritableStream` pipe of 5 000 × 1 KiB chunks — the streams machinery (pure-JS prelude for esrun). |
| **fetch** | 300 sequential GETs against a local HTTP server — the network provider seam end-to-end (started by run.sh via Node; skipped if Node is absent). |
| **rss** | Peak resident set (MB) on the near-empty script — the runtime's memory floor. |

Methodology: `startup`/`bigscript` report the **min** over `STARTUP_RUNS` (the
floor is the launch/parse cost); the other workloads run an untimed JIT warmup,
time themselves with `performance.now()`, and report the **median** of
`WORKLOAD_RUNS` self-timed runs, so a single noisy run can't set the number.
`rss` is sampled with GNU `time` or a `python3` `getrusage` fallback (the row is
omitted if neither is available).

## Representative results

Times in **milliseconds, lower is better** (`rss` in MB). One Linux x86-64 box;
numbers are indicative and will vary by machine — re-run locally for your own.

```
workload    |      node |       bun |      deno |     esrun
-----------+-----------+-----------+-----------+-----------
startup     |      19.1 |       9.2 |      24.4 |       6.6
bigscript   |      30.3 |      22.4 |      35.5 |      18.8
compute     |     215.3 |     130.9 |     229.0 |     252.7
json        |     317.0 |     227.9 |     260.3 |     237.8
jsonbig     |     768.5 |     710.6 |     610.4 |     656.6
sha256      |     683.6 |     539.6 |     596.7 |     364.3
crypto      |     236.9 |     116.0 |     188.9 |      37.3
url         |      60.5 |      87.7 |     116.8 |     106.0
encoding    |      86.5 |      26.1 |      83.6 |      85.0
base64      |       7.9 |      15.4 |       8.2 |      86.2
structured  |     249.6 |     293.7 |     292.6 |     342.9
async       |      70.1 |      56.4 |      36.6 |      38.7
timers      |       9.4 |       8.5 |      29.2 |       5.4
streams     |      26.1 |      22.8 |      17.0 |      12.6
fetch       |     107.8 |      28.7 |      46.5 |      44.3
rss         |        40 |        29 |        54 |        18
```

(node v24, bun 1.3, deno 2.8, esrun 0.0.0.)

## Interpretation

**Where esrun wins or ties:**

- **startup (6.6 ms) — fastest**, despite being a single ~70 MB statically-linked
  binary. Two things pay for it: the **V8 startup snapshot baked into the binary**
  at build time (`build.rs`; the whole prelude pre-executed, restored instead of
  recompiled — ~7 ms off `Runtime::new`'s old cost) and **lazy HTTP-client
  build-out** (the reqwest client, TLS config and root store, is built on first
  `fetch`, not at boot; isolated, the eager client cost ~5.5 ms of startup).
- **bigscript (18.8 ms) — fastest.** The snapshot covers the prelude, not user
  source, so this is real parse work on ~100 KB; the fast process floor carries.
- **sha256, crypto — fastest, by a wide margin on crypto** (37 ms vs Bun's 116).
  In esrun `crypto.subtle.*` is a synchronous RustCrypto op wrapped in an
  already-resolved promise, so the `await`s drain in microtask checkpoints with
  little scheduling cost; Node/Deno/Bun run genuinely-async WebCrypto that pays
  per-call scheduling. A real, explainable win **for this access pattern** — not
  a claim that RustCrypto beats BoringSSL raw.
- **timers, streams — fastest.** The driven loop's timer queue and the pure-JS
  streams prelude both hold up well; nothing pathological in the seam the loop
  exposes.
- **async — second (38.7 ms), ahead of Node and Bun.** The microtask-checkpoint
  integration of the *driven* loop (esrun's distinctive risk) is competitive.
- **rss (18 MB) — lowest footprint** of the four, the 70 MB on-disk binary
  notwithstanding.
- **json, jsonbig — mid-pack and competitive**; pure-engine baselines confirming
  the engine itself isn't a bottleneck in the workloads that wrap it.

**Where esrun trails, and why:**

- **compute (~17% behind Node, same engine).** Flag experiments (`--maglev`,
  `--max-opt`, …) moved nothing — Maglev and concurrent compilation are already
  on. The residual is attributed to the prebuilt `rusty_v8` library's build
  configuration (e.g. pointer compression, which Node builds without) and V8
  version skew — not addressable from this repo. Far behind Bun's JavaScriptCore.
- **url, encoding — competitive but behind the native parsers.** This surface
  crosses the JS↔Rust op boundary per call. It got here through three rounds:
  (1) op *dispatch* is cheap (~49 ns/call) — the cost was always per-call *work*;
  (2) structured marshaling (building a JS object property-by-property) was tried
  and **reverted** — slower than a Rust-side JSON serialize + `JSON.parse`;
  (3) **offsets beat both** — `url_parse` returns the canonical href plus 15
  component offsets as one small array, and every getter is a lazy
  `href.slice(...)` (nothing built for components a script never reads). Encoding
  took the complementary fix: op results are **consumed, not copied** (the byte
  buffer *moves* into the `ArrayBuffer`; `decode()` converts valid UTF-8 in
  place). Bun's lead here is JavaScriptCore's specialized encoder fast paths.
- **base64 (86 ms vs ~8 ms native).** Moving the transcoding loop from a pure-JS
  per-char concatenation into a host op was a ~4.5× win (386 → 86 ms), but two
  op crossings per round trip plus string building still trail the native
  intrinsics. Rarely hot; left as-is.
- **structured (slowest, 343 ms).** `structuredClone` is a pure-JS recursive
  walk in the prelude. Making it a host op would need **structured marshaling of
  arbitrary JS objects across the boundary** — exactly the deferred D3a work; the
  same reason a faster `base64`/`url`/`encoding` eventually wants a zero-copy
  structured path rather than more per-call cleverness.

## Caveats

- These are **microbenchmarks** — they isolate one thing each and don't predict
  whole-application performance.
- esrun runs **single-file classic scripts** (no ES-module loader) and grants all
  capabilities — it's a convenience runner, not a sandbox here.
- The crypto shapes reflect esrun's **op model** (sync ops wrapped in promises)
  as much as the underlying libraries.
- `fetch` hits a trivial local server returning 64 bytes — it measures the
  request/response *plumbing* and the provider seam, not throughput or TLS.
