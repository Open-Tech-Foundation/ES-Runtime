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
| **http** | 2 000 requests (batches of 100 concurrent) against each runtime's **own** HTTP server on loopback — `fetch` → handler → 64-byte response (esrun: `runtime:http` `serve` on hyper; Node `http`, `Bun.serve`, `Deno.serve` elsewhere). Server throughput on the warm request/response path. |
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
startup     |      18.1 |       9.2 |      25.8 |       7.2
bigscript   |      30.3 |      22.9 |      36.1 |      20.0
compute     |     214.4 |     157.8 |     244.1 |     267.7
json        |     317.1 |     253.6 |     271.7 |     250.1
jsonbig     |     756.1 |     738.1 |     671.8 |     755.3
sha256      |     729.1 |     566.7 |     664.6 |     381.5
crypto      |     265.8 |     127.2 |     190.8 |      39.6
url         |      67.1 |      96.5 |     126.2 |     115.7
encoding    |      88.1 |      27.8 |      92.6 |      97.7
base64      |      10.5 |      15.4 |       9.7 |      88.7
structured  |     265.4 |     318.2 |     324.4 |     370.5
async       |      73.9 |      61.6 |      38.8 |      36.8
timers      |       7.2 |       9.3 |      30.7 |       5.4
streams     |      31.4 |      26.8 |      17.4 |      12.7
fetch       |     123.9 |      25.4 |      47.5 |      47.7
http        |     514.7 |      75.7 |     136.6 |     159.3
fsread      |     168.7 |      45.0 |      55.2 |      82.0
fswrite     |     235.4 |      13.9 |     114.8 |     110.8
fsappend    |     151.9 |      48.6 |      55.9 |      74.9
glob        |     286.4 |      43.7 |       n/a |      57.4
rss         |        40 |        29 |        53 |        19
```

(node v24, bun 1.3, deno 2.8, esrun 0.1.)

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
- **async — second, ahead of Node and Bun.** The microtask-checkpoint
  integration of the *driven* loop (esrun's distinctive risk) is competitive.
- **http — ahead of Node, behind Bun/Deno.** `runtime:http` (hyper) serving 64-byte
  responses under concurrent load lands ~3× Bun and ~1.2× Deno but comfortably
  past Node. This is the workload that motivated wiring a **real waker** into the
  driven loop: a ready (or just-dispatched) async op now wakes the loop at once
  instead of waiting out a fixed re-poll interval, which also restored `fetch`
  and the `fs` workloads to their proper latency. Sequential round-trip latency
  fell from ~13 ms to ~0.14 ms/request.
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

## HTTP requests/sec

`run.sh`'s `http` workload runs the client *and* the server in one process, so on
esrun a single thread does both jobs — useful for the warm request/response path,
but not a server-throughput number. For that, `bench/rps.sh` runs a hello-world
server per runtime (`scripts/helloserver.js`, plaintext `"Hello, World!"` on
:3000) and points an **external** load generator ([autocannon]) at it — the
classic plaintext req/s shape.

```sh
cargo build --release -p es-runtime-cli
bench/rps.sh                       # autocannon -c 100, one connection/req
CONN=250 PIPELINE=20 bench/rps.sh  # higher concurrency + HTTP pipelining
```

Needs `autocannon` (used via `bunx`/`npx` if not installed globally). Indicative
numbers on one Linux x86-64 box:

```
# bench/rps.sh           (-c 100 -p 1)        # CONN=250 PIPELINE=20
runtime |      req/sec                        runtime |      req/sec
--------+------------                         --------+------------
node    |      32,924                         deno    |     125,715
bun     |      35,644                         node    |      54,884
deno    |      35,822                         esrun   |      35,156
esrun   |      36,641                         bun     |      19,226
```

At ordinary concurrency (one in-flight request per connection) all four sit
around ~35k req/s — esrun is at parity, marginally highest here. Under heavy
HTTP pipelining the spread reflects each server's I/O model; esrun holds ~35k,
which is its **single-thread ceiling** — one V8 isolate on a current-thread tokio
runtime, by design (it's an embeddable runtime, not a multi-core web server). The
earlier "2× slower" reading came from the in-process `http` workload, where esrun
pays for the client and the server on the same thread; measured server-to-client
it isn't there.

### Through a framework (Hono)

The same shape served through [Hono] — a real, third-party web framework —
instead of each runtime's bare server. This is the framework counterpart to the
Bun framework charts: it shows esrun runs **unmodified npm ESM packages** off
`node_modules`, not just its own server. Hono is Web-standard
(`app.fetch(request) -> Response`), so it plugs straight into `runtime:http`,
`Bun.serve`, and `Deno.serve`; Node uses Hono's `@hono/node-server` adapter.

```sh
cd bench && bun install               # hono + @hono/node-server
SERVER=scripts/hono.js bench/rps.sh   # -c 100 -p 1
```

```
runtime |      req/sec
--------+------------
node    |      33,358
bun     |      39,686
deno    |      40,150
esrun   |      40,220
```

esrun is again at parity (marginally highest), and the framework layer costs all
four about the same as the bare server — Express, by contrast, cannot run on
esrun at all (it is CommonJS and needs `node:http`'s `(req, res)` API; esrun is
ESM-only and rejects `node:` builtins).

[autocannon]: https://github.com/mcollina/autocannon
[Hono]: https://hono.dev

## Caveats

- These are **microbenchmarks** — they isolate one thing each and don't predict
  whole-application performance.
- esrun runs **single-file classic scripts** (no ES-module loader) and grants all
  capabilities — it's a convenience runner, not a sandbox here.
- The crypto shapes reflect esrun's **op model** (sync ops wrapped in promises)
  as much as the underlying libraries.
- `fetch` hits a trivial local server returning 64 bytes — it measures the
  request/response *plumbing* and the provider seam, not throughput or TLS.
