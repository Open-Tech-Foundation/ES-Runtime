# Cross-runtime benchmark

Compares **esrun** (the ES-Runtime CLI) against **Node.js**, **Bun**, **Deno**,
and **LLRT** on a spread of Web-API workloads. Each workload uses only standard
Web APIs, so the same script (`scripts/*.js`) runs unmodified on each runtime;
where a runtime lacks an API the cell is **n/a** (e.g. Deno has no built-in glob;
LLRT has no general HTTP server and only partial `fs`/streams).

[LLRT](https://github.com/awslabs/llrt) (AWS Low Latency Runtime) is QuickJS-based
and built for cold-start and low memory — a deliberate foil for esrun's startup
and footprint numbers, and a different engine (QuickJS, vs V8 for
esrun/Node/Deno and JavaScriptCore for Bun). It runs the engine + Web-API
workloads it supports; `http`/`streams`/`fs`/`glob` fall through to n/a.

## Running

```sh
cargo build --release -p es-runtime-cli   # build esrun first
bench/run.sh                              # auto-detects node / bun / deno / llrt / esrun
```

Knobs (env vars): `ESRUN=/path/to/esrun`, `STARTUP_RUNS` (default 25),
`WORKLOAD_RUNS` (default 5), `WORKLOAD_TIMEOUT` (per-workload cap, default 60s, so
an unsupported workload yields n/a instead of hanging), `WORKLOADS="url encoding"`
(run a subset), `BENCH_JSON=1` (machine-readable output for diffing runs over
time). A runtime that isn't installed is skipped; Deno is also looked for at
`~/.deno/bin/deno` and `/tmp/deno/bin/deno`, and LLRT at `~/.llrt/bin/llrt`,
`~/.local/bin/llrt`, or `/tmp/llrt/llrt` if not on `PATH`. Install LLRT by
unzipping the `llrt-linux-x64.zip` release asset onto your `PATH`.

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
workload    |      node |       bun |      deno |      llrt |     esrun
-----------+-----------+-----------+-----------+-----------+-----------
startup     |      22.5 |       9.8 |      26.1 |       3.3 |       6.8
bigscript   |      31.6 |      22.5 |      35.4 |      12.0 |      19.5
compute     |     232.6 |     134.6 |     246.9 |    2465.0 |     258.6
json        |     319.7 |     248.4 |     254.2 |     789.9 |     251.2
jsonbig     |     792.1 |     686.4 |     636.4 |    1992.0 |     693.3
sha256      |     718.7 |     554.8 |     672.9 |     375.8 |     383.6
crypto      |     249.1 |     125.0 |     185.1 |      28.9 |      39.1
url         |      57.8 |      97.1 |     129.7 |     129.6 |     102.2
encoding    |      86.2 |      29.8 |      86.9 |      84.7 |      86.3
base64      |       9.8 |      18.4 |       9.8 |      36.9 |      72.8
structured  |     266.9 |     309.7 |     312.8 |     363.9 |     342.5
async       |      71.2 |      56.3 |      40.6 |     792.2 |      41.2
timers      |       8.2 |       9.1 |      30.4 |       5.6 |       6.2
streams     |      30.1 |      24.2 |      18.3 |       n/a |      12.8
fetch       |     118.3 |      26.3 |      47.6 |      28.6 |      45.9
http        |     452.2 |      75.5 |     134.6 |       n/a |     110.2
fsread_small |     175.2 |      54.8 |      67.2 |       n/a |      55.0
fsread_large |      25.1 |      12.5 |      29.3 |       n/a |      34.2
fswrite_small |     237.3 |      20.7 |     120.4 |       n/a |     112.7
fswrite_large |      66.8 |      32.5 |      50.0 |       n/a |      47.6
fsappend_small |     186.6 |      57.3 |      65.7 |       n/a |      51.2
fsappend_large |      58.6 |      27.6 |      42.8 |       n/a |      35.5
fsstat_small |      98.3 |      80.8 |     129.8 |       n/a |      80.2
fsstat_large |       1.1 |       0.3 |       1.2 |       n/a |       0.4
fsexists_small |     101.4 |      83.8 |     132.6 |       n/a |      66.2
fsexists_large |       1.1 |       0.3 |       1.0 |       n/a |       0.3
glob        |     289.1 |      47.4 |       n/a |       n/a |      63.6
rss         |        40 |        29 |        53 |        11 |        19
```

(node v24, bun 1.3, deno 2.8, llrt 0.8-beta, esrun 0.2; n/a = API the runtime
lacks. LLRT's QuickJS has no JIT — hence `compute`/`json`/`async` — and no
streams/HTTP-server/`fs` here.)

## Interpretation

**Reading the LLRT column.** LLRT is the cold-start/footprint specialist —
QuickJS, no JIT, trimmed surface — so it leads `startup` and `rss` and stays in
the pack on the synchronous-crypto workloads, but its lack of a JIT shows starkly
on `compute`/`json`/`jsonbig`/`async` (often 5–30×), and it has no streams, HTTP
server, or `fs` here. It's the honest yardstick for esrun's startup/memory
claims: esrun's pitch is **near-LLRT boot with a full JIT engine and the complete
WinterTC surface**, not "fastest at everything."

**Where esrun wins or ties:**

- **startup (6.7 ms) — fastest of the JIT runtimes** (~3.6× under Node/Deno),
  beaten only by LLRT's no-JIT QuickJS (3.4 ms). Two things pay for esrun's:
  the **V8 startup snapshot baked into the binary** at build time (`build.rs`;
  the whole prelude pre-executed, restored instead of recompiled) and **lazy
  HTTP-client build-out** (the reqwest client/TLS/root store is built on first
  `fetch`, not at boot — isolated, the eager client cost ~5.5 ms of startup).
- **bigscript (20 ms) — fastest of the JIT runtimes** (LLRT parses faster, having
  no JIT to feed). Real parse work on ~100 KB; the fast process floor carries it.
- **async, timers, streams — fastest.** The driven loop's microtask-checkpoint
  integration (esrun's distinctive risk), its timer queue, and the pure-JS
  streams prelude all hold up; LLRT's QuickJS microtask path is ~20× slower on
  `async`, and it has no streams.
- **crypto, sha256 — fastest among the JIT runtimes, by a wide margin on crypto**
  (40 ms vs Bun's 112). `crypto.subtle.*` is a synchronous RustCrypto op wrapped
  in an already-resolved promise, so the `await`s drain in microtask checkpoints
  with little scheduling cost; Node/Deno/Bun run genuinely-async WebCrypto that
  pays per-call scheduling. LLRT (also a native synchronous crypto path) lands
  alongside. A real win **for this access pattern** — not a claim that RustCrypto
  beats BoringSSL raw.
- **http — ahead of Node, behind Bun/Deno** (and LLRT has no HTTP server). See
  the **HTTP requests/sec** section below for the server-throughput story
  (per-request CPU cost) — the in-process `http` micro-workload here just exercises
  the warm request/response path.
- **rss (19 MB) — lowest among the JIT runtimes**, under LLRT's 11 MB QuickJS.
- **json, jsonbig — mid-pack and competitive**; pure-engine baselines confirming
  the engine itself isn't a bottleneck (and where LLRT's missing JIT bites hardest).

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
:3000) and points an **external** load generator at it — the classic plaintext
req/s shape.

The generator is [oha] (or [bombardier]) — **not** autocannon: Bun's own
`bench/express` README notes autocannon's node:http client can't push a fast
server hard enough to measure it, and indeed autocannon capped *every* runtime at
~35–40k here, hiding the real spread. Following Bun's setup, we send
`-H "Accept-Encoding: identity"` (so Deno doesn't gzip the body) and a fixed
request count.

```sh
cargo build --release -p es-runtime-cli
cargo install oha                        # or: go install github.com/codesenberg/bombardier@latest
bench/rps.sh                             # oha -c 100 -n 500000
CONN=250 REQUESTS=1000000 bench/rps.sh   # heavier load
```

Indicative numbers on one Linux x86-64 box (12 cores):

```
# bare server (runtime:http)            # through Hono (framework)
runtime |      req/sec                  runtime |      req/sec
--------+------------                   --------+------------
deno    |      85,070                   deno    |      71,531
bun     |      82,615                   bun     |      62,894
esrun   |      49,537                   esrun   |      47,722
node    |      29,558                   node    |      28,217
```

esrun beats Node comfortably and reaches roughly two-thirds of Bun/Deno on the
bare server. **All three (esrun, Bun, Deno) saturate ~one core** under this load,
so this is not a core-count gap but a per-request one.

Wall-clock req/s is noisy on a shared box, though (a busy machine throttles the
single-threaded server unpredictably). The **contention-immune** measure is the
server's **CPU time per request** — what it actually computes, independent of how
long it waited for a core — and it's stable across runs:

```
                 CPU µs/req   ~req/s on 1 core
bare hyper (Rust)    ~10.4         —   (transport floor, no JS)
deno                 ~11.9       ~84k
bun                  ~12.2       ~82k
esrun                ~18.2       ~55k
node                 ~33.8       ~30k
```

The story is in the gap over bare hyper: Bun/Deno add only ~2µs of JS-handler
overhead (their HTTP server calls JS natively); esrun adds ~8µs — the
**injectable-provider + driven-loop seam** (hyper hands each request over a
channel, the JS loop pulls it via an async op/promise, and the response crosses
back the same way). That seam is what makes esrun embeddable and
capability-secured; it isn't waste, it's the boundary. The request path was tuned
hard against it — batched accept (many requests per op crossing), structured
request metadata (no per-request JSON), a synchronous + lazily-encoded response
body, lazy `Headers`, and reusing the host-validated URL — taking esrun from
~29µs to ~18µs CPU/req. The remaining floor is that seam plus the single
V8 isolate on a current-thread tokio runtime — by design (an embeddable runtime,
not a multi-core web server).

### Through a framework (Hono)

The right-hand column above is the same shape served through [Hono] — a real,
third-party web framework — instead of each runtime's bare server. It shows esrun
runs **unmodified npm ESM packages** off `node_modules`, not just its own server.
Hono is Web-standard (`app.fetch(request) -> Response`), so it plugs straight into
`runtime:http`, `Bun.serve`, and `Deno.serve`; Node uses Hono's `@hono/node-server`
adapter.

```sh
cd bench && bun install               # hono + @hono/node-server
SERVER=scripts/hono.js bench/rps.sh
```

The framework narrows the gap (esrun is within ~25% of Bun here), because
`runtime:http` is already esrun's native path while Bun/Deno pay Hono's adapter
cost on top of their fast servers. Express, by contrast, cannot run on esrun at
all (it is CommonJS and needs `node:http`'s `(req, res)` API; esrun is ESM-only
and rejects `node:` builtins).

[oha]: https://github.com/hatoo/oha
[bombardier]: https://github.com/codesenberg/bombardier
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
