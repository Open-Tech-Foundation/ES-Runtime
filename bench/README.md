# Cross-runtime benchmark

Compares **esrun** (the ES-Runtime CLI) against **Node.js**, **Bun**, **Deno**,
and **LLRT** on a spread of Web-API workloads. Each workload uses only standard
Web APIs, so the same script (`scripts/*.js`) runs unmodified on each runtime;
where a runtime lacks an API the cell is **n/a** (e.g. Deno has no built-in glob;
LLRT has no general HTTP server, no `WebAssembly`, and only partial `fs`/streams).

[LLRT](https://github.com/awslabs/llrt) (AWS Low Latency Runtime) is QuickJS-based
and built for cold-start and low memory — a deliberate foil for esrun's startup
and footprint numbers, and a different engine (QuickJS, vs V8 for
esrun/Node/Deno and JavaScriptCore for Bun). It runs the engine + Web-API
workloads it supports; `http`/`streams`/`fs`/`glob`/`fetch_upload` and the
`wasm_*`/`wasi_*` rows fall through to n/a.

## The database benchmark

`bench/db/run.sh` compares **SQLite** across esrun, Node.js, Bun and Deno.

It is separate from `run.sh` because it cannot share a script: esrun's
`runtime:db` is **async over an op boundary**, while `node:sqlite` and
`bun:sqlite` are **synchronous and in-process**. So each runtime gets its own
script (`bench/db/<runtime>.mjs`) against one shared workload definition
(`bench/db/workload.mjs`) — same schema, same row counts, same checksum, which
the runner verifies so a runtime cannot look fast by doing less. LLRT has no
SQLite and is n/a.

```sh
cargo build --release -p es-runtime-cli
bench/db/run.sh                          # everything
REPS=5 bench/db/run.sh                   # more repetitions
WORKLOADS="insert point" bench/db/run.sh
RUNTIMES="esrun bun" bench/db/run.sh
```

Each cell is the **minimum** of `REPS` runs (the suite's convention), with peak
RSS and the user/sys CPU split of that run. Measurement is
`resource.getrusage(RUSAGE_CHILDREN)` via `bench/db/measure.py` — the same
mechanism `run.sh` falls back to when GNU `time` is absent.

## The PostgreSQL driver benchmark

`bench/db/pg/run.sh` compares esrun with `@opentf/esrun-postgres` against
`postgres.js` on Node, Bun and Deno — the acceptance test DECISIONS D56 set for
the Postgres path.

```sh
(cd packages/postgres && bun run build)
PG_URL=postgres://postgres:esrun@127.0.0.1:5433/esrun_test bench/db/pg/run.sh
```

Each runtime uses the idiom its driver user would reach for: the buffering path
for a scan, a cursor only for the streaming workload, where the point is that
memory must not grow with the result. Every workload prints a checksum the
runner compares, so a runtime cannot look fast by doing less.

`bench/db/pg/types-*.mjs` put the **type mappings** side by side: one query of
twenty-one typed columns, run through esrun (in both wire formats),
`postgres.js` on Node and Deno, and Bun's built-in `bun:sql`, each printing
`type value` in the same shape so the results can be diffed. It is how you check
that a driver's binary path agrees with its text path, and where the ecosystem
disagrees with itself.

```sh
PG_URL=… esrun bench/db/pg/types-esrun.mjs binary
PG_URL=… esrun bench/db/pg/types-esrun.mjs text
PG_URL=… node bench/db/pg/types-postgresjs.mjs
PG_URL=… bun  bench/db/pg/types-bunsql.mjs
```

`bench/db/pg/decode-share.mjs` answers a narrower question — what share of a
scan is decoding — by exploiting lazy rows: the same query, touching a different
number of columns, holds the network and the protocol constant.

## The Redis client benchmark

`bench/db/redis/run.sh` compares esrun with `@opentf/esrun-redis` against each
runtime's **own** answer: Bun's built-in `RedisClient`, and `ioredis` on Node and
Deno, which have none. That is the comparison the question deserves — what each
runtime gives you — rather than how one npm package performs on four engines.
`BUN_REDIS=ioredis` switches Bun onto the same library as Node when you want the
like-for-like version instead.

```sh
(cd packages/redis && bun run build)
docker run -d --name esrun-redis-plain -p 6379:6379 redis:8
bench/db/redis/run.sh
```

What it measures is the **client**, not Redis: a command's cost is a round trip
and a decode, and the server's share of that is small. The workloads separate
the two.

| Workload | What it isolates |
| --- | --- |
| `serial_set` / `serial_get` | one command at a time — round-trip bound, the floor every client shares |
| `pipeline` | the same work batched; the gap against `serial_set` is the whole argument for pipelining |
| `list` | one 50 000-element reply — decode bound |
| `hash` | 200 × a 1 000-field map — decode bound, over the shape RESP3 types and RESP2 does not |

Each client uses its own idiom for the batch: a `pipeline()` builder for esrun
and ioredis, `Promise.all` for Bun, whose client pipelines commands issued
together rather than offering a builder. Every workload prints a checksum the
runner compares across runtimes, so a client cannot look fast by doing less.

### Results (min of 5, wall ms)

| Workload | esrun | node+ioredis | bun (built-in) | deno+ioredis |
| --- | ---: | ---: | ---: | ---: |
| serial_set | 899 | 882 | **675** | 887 |
| serial_get | 879 | 903 | **679** | 887 |
| pipeline | 239 | 190 | **73** | 188 |
| list | 164 | 92 | **29** | 96 |
| hash | 980 | 374 | **204** | 370 |

Where the round trip dominates, all four sit within a few percent and Bun's
native client is ~25% ahead — that is the floor. Where **decoding** dominates,
esrun is last: 1.8× behind ioredis on the list scan and 2.6× behind on repeated
`HGETALL`.

The cause is **not** UTF-8 decoding — reading the same reply with
`{ binary: true }`, which skips every `TextDecoder` call, was 2% faster — and not
startup, where esrun is fastest of the four (8 ms against Node's 20 ms). What is
left is allocation in the reply representation: a copied `Uint8Array` per bulk
string and a wrapper object per value, worst for maps, where RESP3's `HGETALL`
builds a pair array before the object the caller asked for.

Peak RSS tells the same story from the other side: esrun is the lightest of the
four on the round-trip workloads (38.9 MB against Node's 76 and Deno's 101) and
the heaviest on the pipelined one.

Unlike `bench/run.sh`, these runs are not interleaved — each runtime is measured
in turn — so small differences are noise and the order of magnitude is the
finding.

## Running

```sh
cargo build --release -p es-runtime-cli   # build esrun first
bench/run.sh                              # auto-detects node / bun / deno / llrt / esrun
bench/rps.sh                              # server throughput, external load generator
bench/http2.sh                            # the same server over HTTP/1.1 vs HTTP/2
```

### Scoped runs

The full suite takes a while, and most work touches one area of it. Rows are
grouped, and a run can be scoped to any of them:

```sh
bench/run.sh --list                       # the groups, and any row no group claims
GROUP=fs bench/run.sh                     # just the filesystem rows
GROUP="engine crypto" bench/run.sh        # several groups
WORKLOADS="regex strings" bench/run.sh    # or name rows directly
```

| Group | Rows |
| --- | --- |
| `launch` | `startup`, `bigscript`, `modules` (+ the derived `rss`, `rss_loaded`) |
| `memory` | `rss_load` |
| `engine` | `compute`, `json`, `jsonbig`, `regex`, `strings`, `structured`, `errors`, `async`, `timers` |
| `webapi` | `url`, `url_setter`, `urlpattern`, `encoding`, `base64`, `buffers`, `headers`, `formdata`, `date_intl`, `streams`, `compression` |
| `crypto` | `sha256`, `crypto`, `crypto_asym`, `crypto_kdf` |
| `net` | `fetch`, `fetch_upload`, `http`, `websocket` |
| `fs` | `fsread_*`, `fswrite_*`, `fsappend_*`, `fsstat_small`, `fsstat_many`, `fsexists_small`, `fsexists_many`, `glob` |
| `system` | `spawn` |
| `serialization` | `jsonl_stream`, `xml_*`, `yaml_*`, `toml_*`, `msgpack_*` |
| `protobuf` | `protobuf_small`, `protobuf_large` |
| `wasm` | `wasm_compile`, `wasm_call`, `wasm_mem` |
| `wasi` | `wasi_start`, `wasi_syscall` |

`WORKLOADS` wins over `GROUP` when both are set. The variable is `GROUP`, not
`GROUPS` — bash reserves `GROUPS` for the caller's group IDs, so it can never be
set from the environment. Every row must belong to
exactly one group; `--list` reports anything in `scripts/` that none claims,
which is how a newly added workload gets noticed rather than silently never
running. Publishing to the site still requires a full run — a scoped run is for
working, not for regenerating.

### The row catalogue

`ROW_DEFS` in `run.sh` is the single definition of a row — group, unit, where it
is shown, and what to call it:

```
group | key | unit | display | label
```

`display` is `card` (charted on the benchmarks page and carded on the home-page
roller), `chart` (benchmarks page only), or `hidden` (measured, shown nowhere —
`rss_load` exists to produce the memory numbers, not to be read itself).

`BENCH_JSON` publishes the whole catalogue as `rows` and `groups`, on every run
including a scoped one, and the site renders from it: the benchmarks page asks
for a group and gets whatever that group holds, the roller asks for the `card`
rows, and `metric-direction.js` reads `better` from it. No component names a
metric, a label, a unit or an order. Adding a line to `ROW_DEFS` is the whole of
adding a row to the site — and `validate-bench-data.mjs` will not publish a run
whose rows the benchmarks page does not reach.

Knobs (env vars): `ESRUN=/path/to/esrun`, `STARTUP_RUNS` (default 15),
`WORKLOAD_RUNS` (default 5 — a *ceiling*, see the adaptive stop below),
`MIN_REPS` (repetitions before a row may stop early, default 3),
`NOISE_THRESHOLD` (CoV % above which a cell is flagged noisy, and below which a
row counts as settled, default 5), `RSS_ROWS` (rows to sample peak memory
for, default: every row — set it to a short list, e.g. `RSS_ROWS="startup
rss_load"`, to skip the extra launch per row while iterating), `MIN_MS` (below this a cell is reported as being
under the measurement floor, default 5), `WORKLOAD_TIMEOUT` (per-workload cap,
default 60s, so an unsupported workload yields n/a instead of hanging),
`GROUP="fs net"` / `WORKLOADS="url encoding"` (scoped run, see above), `QUIET=1` (pin to one CPU + disable
ASLR for lower variance; see Methodology), `BENCH_CPU` (the core to pin under
`QUIET`, default 0), `BENCH_JSON=1` (machine-readable output for diffing runs
over time). A runtime
that isn't installed is skipped; Deno is also looked for at `~/.deno/bin/deno`
and `/tmp/deno/bin/deno`, and LLRT at `~/.llrt/bin/llrt`, `~/.local/bin/llrt`, or
`/tmp/llrt/llrt` if not on `PATH`. Install LLRT by unzipping the
`llrt-linux-x64.zip` release asset onto your `PATH`.

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
| **urlpattern** | 50 000 × `new URLPattern(...)` + `.test()` matches — polyfilled inside V8 vs native. |
| **encoding** | 100 000 × `TextEncoder`/`TextDecoder` UTF-8 round trips — op crossings riding V8's native transcoding. |
| **base64** | 10 000 × `btoa`/`atob` of a 1 KiB string — op-backed for esrun; native elsewhere. |
| **structured** | 50 000 × `structuredClone` of a nested object — pure-JS recursive clone for esrun. |
| **async** | 1 000 000 × `await Promise.resolve(...)` — the microtask machinery and (for esrun) the driven loop's checkpoint. |
| **timers** | 10 000 zero-delay `setTimeout`s drained to completion — timer scheduling + driver. |
| **streams** | `ReadableStream`→`TransformStream`→`WritableStream` pipe of 5 000 × 1 KiB chunks — the streams machinery (pure-JS prelude for esrun). |
| **fetch** | 300 sequential GETs against a local HTTP server — the network provider seam end-to-end (started by run.sh via Node; skipped if Node is absent). |
| **fetch_upload** | 200 sequential POSTs each streaming an 8 KiB `ReadableStream` request body (chunked upload) to the same local server — the request-body streaming path: building the body stream, the per-chunk host channel with backpressure, and chunked transfer-encoding. The server echoes the bytes it received and the workload **verifies** them, so a runtime that doesn't truly stream the body (e.g. LLRT, which coerces the stream) is recorded **n/a** rather than posting a misleadingly fast time. |
| **http** | Client and server in the **same process**, so it measures the server together with that runtime's `fetch` — `bench/rps.sh` is the server-alone number. 2 000 requests (batches of 100 concurrent) against each runtime's **own** HTTP server on loopback — `fetch` → handler → 64-byte response (esrun: `runtime:http` `serve` on hyper; Node `http`, `Bun.serve`, `Deno.serve` elsewhere). Server throughput on the warm request/response path. |
| **websocket** | 20 000 serial message round-trips over one `WebSocket` to a local echo server — the WebSocket *client* seam: opening handshake then per-message `send` + event dispatch (esrun: the `ws_send` op + the receive-pump's `MessageEvent` per tick). Server is whichever built-in WS server is present (Bun/Deno, or Node + `ws`); LLRT has no `WebSocket`, hence n/a. |
| **fsread_small / _large** | 2 000 reads of a 4 KB file / 20 reads of a 2 MB file. |
| **fswrite_small / _large** | The same shape for whole-file writes. |
| **fsappend_small / _large** | 2 000 × 4 KB appends / 60 × 256 KB appends to a growing file. See the sizing note in `fsappend_large.js`: this row is squeezed between the kernel's dirty-page threshold above and the measurement floor below. |
| **fsstat_small / fsstat_many** | 5 000 `stat`s of one path / 20 rounds of `stat` across 1 000 distinct paths. The second is a directory's worth of dentries rather than one cached entry — what a static-file server or module resolver does. |
| **fsexists_small / fsexists_many** | The same two shapes for existence checks, each via the runtime's idiomatic API — `access()` on Node and LLRT, `Bun.file().exists()` on Bun, `runtime:fs` `exists()` on esrun. Deno ships no existence primitive, so that cell alone is a `stat`. Using `stat` everywhere (as this row once did) made it a near-duplicate of the fsstat rows and hid that Bun's native check is ~7x its stat path. |
| **glob** | 200 × `**/*.txt` scans of a generated 10×10 tree. Deno has no built-in runtime glob, hence n/a. |
| **wasm_compile** | 60 × `WebAssembly.compile` of a ~250 KB module (600 functions) — validation + codegen. Each module carries a different salt so the bytes differ and no compilation cache can serve the result. |
| **wasm_call** | 20M calls across the JS↔wasm boundary into an exported `add`, plus 100M iterations of the same arithmetic run *inside* wasm — separates per-call boundary cost from wasm execution. |
| **wasm_mem** | 8 000 × (JS fills a 64 KiB window of the instance's linear memory through a typed array; wasm sums it back) — the shared-buffer shape most real wasm interop takes. |
| **wasi_start** | 2 000 × (construct a `WASI`, instantiate a command module against its import object, run `_start`) on a pre-compiled module — what *running a `wasm32-wasip1` program* costs per invocation. The guest makes no syscalls. |
| **wasi_syscall** | A guest whose `_start` loops 60 000 times calling `random_get` + `clock_time_get`, timed around `start()` alone — the preview-1 implementation on the host side, called from inside wasm where a real program calls it. |
| **modules** | Process wall-time loading a generated **300-module graph** (flat fan-out from an entry, plus a shared util every module imports). `bigscript` measures parse throughput on one big file; this measures resolution, per-module instantiation and linking, which is what a real cold start is mostly made of. |
| **regex** | 200 000 × (route match + field validation + global replace) — the engine's regex implementation (Irregexp for esrun/Node/Deno, JavaScriptCore's for Bun), which runs on essentially every inbound request in real code. |
| **strings** | 100 000 × (template interpolation, rope-building concatenation, header split/trim, search + slice, case fold) — string internals, plausibly the most-executed shape of code in a web server. |
| **errors** | 100 000 × throw/catch across three frames **including reading `.stack`** — the unwind plus the stack capture, which is the expensive half and the one a failing endpoint pays. |
| **buffers** | 20 000 × (4 KiB `TypedArray` copy, big-endian `DataView` field read/write, subarray view) over a 64 KiB block — the layer every binary protocol sits on. |
| **headers** | 50 000 × (build a 7-header `Headers`, case-insensitive reads, `append`/`set`, full iteration, then the same set through a `Request`) — header handling runs on every request a server answers, and a case-insensitive multi-map with ordering rules is more work than it looks. |
| **formdata** | 2 000 × (encode a `FormData` with three fields and a 4 KiB file into a request body, then parse it back with `Request.formData()`) — the multipart path a file upload takes, and the parse half is what runs on untrusted input. |
| **date_intl** | 50 000 × (`Intl.DateTimeFormat` + `Intl.NumberFormat` + `toISOString`) with formatters constructed once — the ICU-backed surface, which a runtime may bundle, trim, or omit entirely. |
| **crypto_asym** | 2 000 × ECDSA P-256 sign + verify — public-key work, a different backend from the symmetric rows and the shape of signing or checking a token per request. |
| **crypto_kdf** | 20 × PBKDF2-HMAC-SHA-256 at 10 000 iterations — a KDF is deliberately slow, so per-call overhead vanishes and this is nearly pure backend hash-loop throughput. Iteration count is well below a production setting; it is a comparison, not a security recommendation. |
| **spawn** | 200 × (start `/bin/echo`, drain its stdout, wait for exit) — `Deno.Command`, `Bun.spawn`, `node:child_process` `spawn`, and esrun's `runtime:system` `Command`. `/bin/echo` rather than `/bin/true` because wiring up the output pipe and handing the bytes back is the runtime's half of the cost; fork/exec is the kernel's. Node's branch uses `spawn` rather than `execFile` because LLRT ships only the former, and reaching for the wrapper would have recorded LLRT as unable to start a process at all. |
| **rss_load** | Builds a 200 000-entry retained working set while churning short-lived objects against it — allocation and collection cost with a mostly-live heap. Its **peak RSS** is the point (published as `rss_loaded`); the elapsed time is reported too. |
| **rss** | Peak resident set (MB) on the near-empty script — the runtime's memory **floor**. |
| **rss_loaded** | Peak resident set (MB) during `rss_load` — memory while something is actually retained. `rss` is the number a runtime looks best on; this is the one that decides whether a box stays up. |

**What the fs rows are and are not.** Every file these workloads touch is
written moments before it is read and is far smaller than available RAM, so the
reads are served from page cache and nothing here calls `fsync`. That means they
measure the **runtime's** cost above the syscall — path resolution, the JS↔native
boundary, buffer allocation, whether an fd is reopened per call, threadpool
versus `io_uring` — and not disk speed or durability. That is the right quantity
for comparing runtimes, since the disk is a constant they all share, but it is
not "how fast is file I/O": a durability-bound workload with `fsync` in it would
rank these differently. The absolute numbers are also specific to the filesystem
they ran on — `results.environment.filesystem` records which — and ext4, tmpfs
and a Docker overlay are three different answers.

The wasm modules are **assembled in JS** (`scripts/wasm-mod.js`) rather than
checked in as `.wasm` fixtures, so every runtime compiles byte-identical input
and a workload can vary a constant per iteration. `WASI` is taken from
`runtime:wasi` on esrun and `node:wasi` on Node/Bun/Deno (Bun exposes the import
object as `wasi.wasiImport` rather than `getImportObject()`; both are handled).
LLRT has no `WebAssembly` at all, so all five rows are **n/a** there.

### Methodology

Designed so contention can't bias the *relative* ranking — the real winner wins
run to run (see Sources for the rationale):

- **Interleaved + randomized.** Each repetition samples every runtime once per
  row back-to-back, with the runtime order shuffled. All candidates therefore
  share the same contention window, instead of one runtime being measured
  minutes after another. This is the key fix: it makes interference hit every
  runtime equally, so close calls aren't decided by *when* a runtime ran.
- **Warmup.** Each script does an untimed in-process warmup so the JIT reaches
  steady state; on top of that the first whole repetition is discarded
  (process-level warmup — fills caches, lets the OS settle). The in-process
  warmup is a tenth of the timed run (never fewer than 5 iterations), the same
  ratio everywhere. This matters more than it looks: the serialization workloads
  once warmed for a flat 5 iterations before 500–1000 timed ones, which cost the
  JIT-backed parser libraries ~10% and the native parsers nothing — a systematic
  tilt toward whichever runtime needed no warmup at all.
- **Each runtime uses the best facility it ships.** Where a workload is not a
  shared Web API — filesystem, HTTP server, YAML/TOML/XML/MessagePack — every
  runtime gets its own native surface: `Bun.YAML`/`Bun.TOML`/`Bun.Glob`,
  `Deno.serve`, `llrt:xml`, esrun's `runtime:serialization`. Only where a runtime
  genuinely ships nothing does it fall back to the library you would reach for
  anyway (`js-yaml`, `@iarna/toml`, `fast-xml-parser`, `msgpackr` on Node and
  Deno). Holding a runtime to a library it does not need understates it — Bun's
  native TOML is ~3x its `@iarna/toml` number — and turns a native-vs-library
  row into a claim about the runtime. Those rows are labelled as such on the site.
- **Min, not median/mean.** Interference only ever *adds* time, so the minimum
  over repetitions is the contention-free floor — the stablest, fairest
  comparator. `startup`/`bigscript` use process wall-time (the launch/parse cost
  is the metric); the other workloads time themselves with `performance.now()`
  and report `RESULT_MS`, isolating engine cost from process launch.
- **Memory is measured per row, not just at startup.** Every workload is run once
  more under GNU `time -v` (or a `getrusage` fallback) to record peak resident
  set, so each cell reports what the work cost in RAM as well as in time — the
  half of the question a server operator is usually asking. Peak RSS is a floor
  that contention cannot inflate, so one sample suffices. This costs an extra
  launch per row per runtime, about 25% more launches; `RSS_ROWS` narrows it.
- **Noise is disclosed, not hidden.** The coefficient of variation (CoV) per
  cell is computed; cells above `NOISE_THRESHOLD%` are marked `~` and listed, so
  a wobbly number is never read as precise. `BENCH_JSON` publishes each cell's
  CoV and sample count next to its value, so the site can show how firm a number
  is instead of implying they are all equally firm.
- **Adaptive stop.** `WORKLOAD_RUNS` is a ceiling, not a quota: once every live
  cell in a row is within `NOISE_THRESHOLD%`, further repetitions cannot move a
  minimum that has already settled, so the row stops (never before `MIN_REPS`).
  Stability is judged per row rather than per cell, so all runtimes keep an
  identical sample count and the interleaving stays balanced. Startup rows are
  exempt — their launches are cheap and their wall-clock measurement is the
  noisiest, so they always take the full `STARTUP_RUNS`.
- **A failure is retried before it is believed.** A cell that fails on the
  warmup repetition is sampled once more before being written off, and a
  timeout is recorded separately from an unsupported API. Both reach the site as
  `null`, but only one of them means "this runtime cannot do this", and a
  transient (a busy port, a cold cache, a timeout tripped under load) used to be
  published as the other.
- **A measurement floor.** A workload whose fastest runtime finishes in under
  `MIN_MS` is measuring timer resolution, not the runtime, and is reported so
  its `N` can be raised rather than its ranking published. This is why
  `fsstat_large`/`fsexists_large` no longer run 20 operations: they inherited
  that count from the read/write `_large` workloads, where 20 ops move 40 MB,
  but a metadata call moves no bytes — so they were landing at 0.2–1.2 ms.
- **Optional hardening (`QUIET=1`).** Pins every runtime to the same CPU
  (`taskset`), disables ASLR (`setarch -R`), and raises priority as root, so all
  candidates face identical conditions. For the lowest variance also set the
  `performance` governor and disable turbo/boost (needs sudo; printed as a hint),
  and close background apps.

`rss` is the memory floor: one sample per runtime via GNU `time` or a `python3`
`getrusage` fallback (the row is omitted if neither is available).

#### Sources

- Kalibera & Jones, [*Rigorous Benchmarking in Reasonable Time*](https://kar.kent.ac.uk/33611/45/p63-kaliber.pdf) (2013) — multi-level repetition, steady state.
- Barrett et al., [*Virtual Machine Warmup Blows Hot and Cold*](https://arxiv.org/pdf/1602.00602) — JIT VMs may never reach a stable steady state.
- [hyperfine](https://github.com/sharkdp/hyperfine) — warmup runs, min/mean/stddev, outlier detection.
- [google/benchmark — *Reducing Variance*](https://github.com/google/benchmark/blob/main/docs/reducing_variance.md) and [pyperf — *Tune the system*](https://pyperf.readthedocs.io/en/latest/system.html) — governor, turbo, pinning, ASLR.

## Representative results

<!-- generated: bench/sync-readme-table.mjs — do not edit by hand -->

Times in **milliseconds, lower is better** (`rss`/`rss_loaded` in MB), from the
same run that feeds the site. One machine; re-run locally for your own numbers.

```
workload      |     node |      bun |     deno |     llrt |    esrun
--------------+----------+----------+----------+----------+----------
startup       |     17.2 |     10.9 |     23.2 |      3.4 |      7.8
bigscript     |     28.5 |     22.8 |     32.1 |     11.1 |     18.7
modules       |     77.0 |     27.5 |     40.8 |     13.8 |     23.7
compute       |    193.2 |    109.2 |    213.1 |   2041.1 |    233.8
json          |    261.2 |    178.4 |    192.7 |    630.4 |    183.7
jsonbig       |    653.7 |    449.4 |    502.3 |   1666.1 |    564.3
regex         |     65.2 |     19.4 |     62.1 |   1151.3 |     61.0
strings       |     59.7 |     76.2 |     58.1 |    147.4 |     56.9
structured    |    209.0 |    263.5 |    250.2 |    313.7 |    295.1
errors        |   1393.3 |    353.7 |   4171.7 |    307.0 |    380.5
async         |     57.0 |     49.5 |     32.1 |    675.9 |     28.9
timers        |     43.4 |     30.4 |    199.1 |     46.5 |     52.1
url           |     48.6 |     70.0 |     98.2 |    110.6 |     86.7
url_setter    |    123.2 |    251.1 |    186.5 |    109.6 |    178.8
urlpattern    |    387.2 |    690.9 |   4812.4 |      n/a |    831.1
encoding      |     68.5 |     22.8 |     69.5 |     70.9 |     80.2
encoding_large|    301.6 |     67.9 |    234.3 |    213.4 |    276.1
base64        |      7.0 |     13.7 |      7.4 |     32.7 |     22.4
buffers       |     13.6 |     20.0 |     12.4 |     73.3 |     12.3
headers       |    440.3 |    268.8 |   1501.6 |    724.6 |    420.5
formdata      |    297.6 |     19.0 |    383.0 |   1345.0 |     86.4
date_intl     |    131.1 |     76.7 |    136.4 |      n/a |    144.0
streams       |     22.2 |      8.2 |     15.3 |      n/a |     10.0
compression   |    645.7 |    240.1 |    220.3 |      n/a |     70.5
sha256        |    530.0 |    419.4 |    475.5 |    328.3 |    335.6
crypto        |    173.1 |     82.9 |    131.8 |     24.8 |     32.7
crypto_asym   |    331.8 |    198.1 |   2159.7 |   1029.2 |   1109.2
crypto_kdf    |     71.3 |     64.8 |     71.0 |    100.5 |     97.9
fetch         |     89.3 |     19.0 |     34.2 |     18.5 |     42.7
fetch_upload  |    107.7 |     41.2 |     35.0 |      n/a |     39.7
http          |    379.7 |     51.2 |    103.3 |      n/a |    108.1
websocket     |    609.9 |    417.8 |    583.0 |      n/a |    743.3
fsread_small  |    115.4 |     36.5 |     40.6 |     26.3 |     39.2
fsread_large  |     61.9 |     24.1 |     64.9 |     13.6 |     67.8
fswrite_small |    159.9 |     12.6 |     83.7 |     90.3 |     68.5
fswrite_large |     57.2 |     18.6 |     40.6 |     52.7 |     20.8
fsappend_small|    105.5 |     30.2 |     39.2 |      n/a |     30.0
fsappend_large|     22.2 |      6.6 |     15.6 |      n/a |      5.8
fsstat_small  |     69.2 |     49.4 |     90.2 |     42.2 |     65.4
fsstat_many   |    271.7 |    200.5 |    374.5 |    171.0 |    289.1
fsexists_small|     69.3 |      7.3 |     91.3 |     49.3 |     52.0
fsexists_many |    265.9 |     30.9 |    371.7 |    203.4 |    227.2
glob          |    204.5 |     29.7 |      n/a |      n/a |     49.6
spawn         |    198.0 |     97.8 |    104.1 |     89.0 |     79.2
jsonl_stream  |    618.7 |    800.9 |    674.1 |      n/a |    609.1
xml_small     |    483.9 |    452.9 |    486.3 |     60.4 |    159.0
xml_large     |    988.7 |    860.4 |    968.5 |    125.3 |    338.5
yaml_small    |    186.0 |     96.6 |    179.1 |   4323.3 |    221.5
yaml_large    |    384.5 |    192.3 |    363.9 |   8672.4 |    440.5
toml_small    |    203.5 |     53.4 |    211.2 |   4114.8 |    158.0
toml_large    |    419.9 |    105.9 |    439.0 |   8351.1 |    325.2
msgpack_small |     40.6 |     62.3 |     38.4 |   1098.8 |     47.4
msgpack_large |     42.2 |     57.0 |     39.5 |   1105.5 |     51.8
protobuf_small|    115.9 |    109.3 |    180.6 |   1795.5 |     75.4
protobuf_large|    568.6 |    504.3 |    939.7 |   8846.3 |    414.8
wasm_compile  |     45.9 |     65.8 |     35.2 |      n/a |     38.4
wasm_call     |     88.3 |    147.1 |     77.7 |      n/a |     78.9
wasm_mem      |    201.9 |    361.4 |    236.1 |      n/a |    238.7
wasi_start    |    268.6 |    591.9 |     43.9 |      n/a |     46.0
wasi_syscall  |     43.7 |   4683.2 |     16.9 |      n/a |     51.3
rss           |     41.0 |     22.0 |     54.0 |     11.0 |     23.0
rss_loaded    |    132.0 |    162.0 |    147.0 |    156.0 |    103.0
```

Intel(R) Core(TM) i7-8700K CPU @ 3.70GHz, 12 cores, Linux 6.12.74+deb13+1-amd64 x86_64, ext2/ext3.

Measured: node v24.14.0, bun 1.4.0-canary.1, deno 2.8.3, llrt v0.8.0-beta, esrun 0.17.0. `n/a` = an API the runtime lacks, or a row it timed out on.

<!-- /generated -->

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
- **wasi_start — fastest, tied with Deno** (48 ms vs Node's 283 and Bun's 641 for
  2 000 program runs). `runtime:wasi` is pure JS in the prelude, so constructing a
  WASI and building its import object is object allocation inside the isolate —
  no host crossing, no native binding to set up per instance.
- **wasm_call — fastest** (80 ms), marginally ahead of Deno and Node; the JS↔wasm
  boundary is V8's own and esrun adds nothing to it.
- **json, jsonbig — mid-pack and competitive**; pure-engine baselines confirming
  the engine itself isn't a bottleneck (and where LLRT's missing JIT bites hardest).

**Where esrun trails, and why:**

- **compute (~15% behind Node, same engine) is entirely `Math.log`.** Splitting
  the row's loop into its parts settles it: `Math.sqrt` costs 27.3 ms on esrun
  against Node's 27.0 and Deno's 27.6, integer work 11.2 against 11.3 and 11.0,
  float multiply 40.8 against 41.0 and 40.7 — identical, to the tenth of a
  millisecond. `Math.log` alone is 221.9 against 167.8 and 176.0, and it is 94%
  of the row.

  So this is one transcendental function in the V8 build we consume, not
  anything about how esrun runs JS. Flag experiments (`--maglev`, `--max-opt`, …)
  moved nothing, which fits — there is no codegen difference to find. The three
  runtimes are on three V8 versions (Node 13.6, Deno 14.9, ours 15.0) and the
  cost tracks the version. Not addressable from this repo, and worth knowing
  before anyone reads `compute` as a general engine verdict: at this workload's
  mix it is a `Math.log` microbenchmark.
- **wasm_compile — was 144 ms against Deno's 36; now 40.** This was previously
  recorded here as wasm codegen rather than the async pipeline, on the evidence
  that the *synchronous* `new WebAssembly.Module` was also several times slower.
  That inference was wrong: sync compilation forgoes V8's background threads, so
  it measures something else, and the async row's gap was the driver parking its
  1 ms fallback on every compile while waiting for a completion V8 announces
  through a foreground task that touches no waker. Fixed in the driver; esrun now
  lands level with Deno and ahead of Node.
- **wasi_syscall (56 ms vs Deno's 17).** Each preview-1 call is a JS function
  reached from wasm that writes its result into linear memory through a
  `DataView`; Node and Deno drop into native implementations. It is the price of
  D34's pure-JS, no-new-attack-surface `runtime:wasi`, and it still beats Bun's
  `node:wasi` by ~86×. The syscalls a real guest makes in bulk are the file ones,
  which go through host ops, not this path.
- **wasm_mem — mid-pack** (244 ms, level with Deno, ahead of Bun). The work is
  V8's; nothing here is ours to win.
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

  A fourth round addressed the **setters** specifically. The JS `URL` holds only
  its href, so every `u.hostname = ...` re-parsed the whole URL to apply one
  change — 0.44µs of parse against 0.56µs of doing the work. The host now keeps a
  bounded cache of parsed URLs keyed by their own serialization, and a setter puts
  its result back, so the next setter on the same object finds it already parsed.
  `url_setter` 263 → 183 ms, past Deno and Bun. `href -> Url` is a pure function,
  which is what makes this safe: a hit cannot give a different answer from a miss.
  Handles were considered and rejected — they buy the same speed while making
  every `new URL()` allocate host state reclaimed only when a
  `FinalizationRegistry` callback happens to run.
- **base64 (22 ms vs ~7 ms native).** Moving the transcoding loop from a pure-JS
  per-char concatenation into a host op was a ~4.5× win (386 → 86 ms); dropping
  the byte-at-a-time whitespace strip in `atob` took it to 22. What remains is two
  op crossings per round trip and the copy through a Rust `String` in each
  direction: 0.73µs for `btoa` of 1 KiB against Node's 0.34. Closing that needs
  ops that read and write V8 strings directly rather than through `Value`, which
  is the same zero-copy structured path `structured` wants. Two things were tried
  and measured *not* to help, so they are not worth retrying: copying small
  results into a V8-allocated `ArrayBuffer` instead of donating the `Vec`, and
  returning `atob`/`btoa` output as Latin-1 bytes — `rusty_v8`'s `String::new`
  already detects ASCII and builds a one-byte string directly.
- **structured (slowest, 343 ms).** `structuredClone` is a pure-JS recursive
  walk in the prelude. Making it a host op would need **structured marshaling of
  arbitrary JS objects across the boundary** — exactly the deferred D3a work; the
  same reason a faster `base64`/`url`/`encoding` eventually wants a zero-copy
  structured path rather than more per-call cleverness.

## HTTP requests/sec

`run.sh`'s `http` workload runs the client *and* the server in one process, so on
esrun a single thread does both jobs — useful for the warm request/response path,
but not a server-throughput number. For that, `bench/rps.sh` runs a hello-world
server per runtime (`scripts/helloserver.js`, plaintext `"Hello, World!"` on a
free port picked per run) and points an **external** load generator at it — the
classic plaintext req/s shape.

The client and the server share one machine, so **the cores are split between
them**: the server gets the lower half, the load generator the upper half
(`taskset`). Unpinned, `oha` starts a worker per core and competes with the very
server it is measuring, and past some rate the number describes whichever won —
the failure mode where every runtime lands within a few percent of every other.
`PIN=0` disables the split; `SERVER_CPUS=0-3 LOAD_CPUS=4-11` chooses it. The
split in force is recorded in the `BENCH_JSON` output next to the numbers.

Results are published under a key derived from `$SERVER`, so a run of the
default `helloserver.js` cannot land in the site's Hono row. Each cell also
reports the **spread** (worst-to-best across the repetitions): a best-of-N with a
wide spread is a lucky draw rather than a ceiling, and the winning number alone
cannot tell you which you have.

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

**Port 3000 must be free.** The hello-world servers bind it directly, so anything
already listening there (a dev server, a stray run) would be load-tested *in place
of* every runtime — which shows up as all runtimes scoring identically. `rps.sh`
refuses to start in that case and names the process holding the port.

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

### HTTP/1.1 vs HTTP/2

`bench/http2.sh` measures the same hello-world server over HTTP/1.1 and over
cleartext HTTP/2 (h2c by prior knowledge). It runs two client shapes, because an
HTTP/2 number in isolation says nothing — it is dominated by how many connections
the client opened and how many streams it put on each.

| shape | client | what it answers |
| --- | --- | --- |
| **wide** | 50 connections × 1 stream | throughput at ordinary load-generator concurrency. HTTP/1.1's best case — it already has 50 sockets, so h2 adds framing cost and buys nothing. |
| **narrow** | 1 connection × 50 streams | multiplexing. HTTP/1.1 on one connection is strictly serial (the next request is written only after the previous response is read); HTTP/2 carries all 50 at once on the same socket. |

Sampling follows the same methodology as `run.sh` rather than inventing a second
one: **interleaved and shuffled** (each repetition samples every runtime back to
back in random order, so contention hits them all in the same window), a
**discarded warmup repetition**, and **best of N** — interference only ever
subtracts throughput, so the maximum is the contention-free ceiling, by the same
argument that makes `run.sh` take the minimum of a duration.

```sh
cargo build --release -p es-runtime-cli
cargo install oha                        # bombardier cannot drive cleartext h2
bench/http2.sh
CONN=250 PARALLEL=100 REPS=5 bench/http2.sh
```

Same box as above (Linux x86-64, 12 cores), `-n 100000`, best of 3:

```
        | wide: 50 conns × 1 stream  | narrow: 1 conn × 50 streams
runtime |  HTTP/1.1    HTTP/2   gain |  HTTP/1.1    HTTP/2   gain
--------+----------------------------+----------------------------
node    |     36,597    18,413 0.50x†|    23,221    39,700 1.71x†
bun     |    119,785    43,086 0.36x†|    31,109    49,142 1.58x†
deno    |    115,141    27,409  0.24x|    32,303    39,209  1.21x
esrun   |     66,939    53,080  0.79x|    20,157    73,541  3.65x
```

#### What this table does and does not license you to compare

- **Down a column: fair.** One client shape, one load generator, each runtime on
  its best available server. On the narrow shape esrun serves **73,541 req/s over
  HTTP/2 — the fastest of the four outright**, 1.50× the next best (Bun, 49,142),
  and it does that while being *slowest* of the four on the same shape over
  HTTP/1.1. That is the claim worth making, and it does not depend on any ratio.
- **The gain column, unmarked rows (esrun, Deno): fair.** Both numbers come from
  one server with one code path, so the ratio isolates the protocol.
- **The gain column, † rows (Node, Bun): not comparable with the others.** For
  both, cleartext h2 lives behind `node:http2` while their default server
  (`node:http`, `Bun.serve`) is HTTP/1.1-only — checked directly with
  `curl --http2-prior-knowledge`. So their ratio carries the gap between two
  *implementations* on top of the protocol change. Bun's 0.36× in particular is
  mostly `node:http2` against a very fast native `Bun.serve`, not a statement
  about HTTP/2.
- **Ratios across runtimes: don't.** esrun's 3.65× is the largest partly because
  its single-connection HTTP/1.1 baseline is the *weakest* of the four (20,157),
  and a small denominator inflates a multiple. The absolute number above is the
  honest version of the same result.

Both halves of the table are expected, and they say opposite things:

- **Wide is HTTP/2's worst case and every runtime loses there** (0.24–0.79×).
  With 50 sockets already open there is nothing to multiplex, so h2 is pure
  overhead: framing, HPACK state, flow-control accounting.
- **Narrow is what HTTP/2 is for.** One connection, 50 requests in flight:
  HTTP/1.1 serialises them, HTTP/2 does not. esrun exploits it well because the
  request handoff is per *request* rather than per connection — responses were
  already matched by request id and could always complete out of order, so
  multiplexing needed no new machinery. It is also the column that matters behind
  a proxy or an API gateway, which is exactly the deployment that holds one
  long-lived connection to the origin.

A cell reads n/a when a runtime cannot serve that version at all, or when no
repetition came back ≥99% successful — a half-failing run is not a throughput
measurement.

[oha]: https://github.com/hatoo/oha
[bombardier]: https://github.com/codesenberg/bombardier
[Hono]: https://hono.dev

### Sustained load

`REQUESTS` fires a fixed burst, which answers "how fast is it when fresh".
`DURATION=60s bench/rps.sh` holds load for a wall-clock window instead, which
answers whether it stays that way once the heap has filled and the allocator has
been churning. Comparing the two is the degradation check, and the site shows it
as a burst-vs-sustained table:

```sh
SECTIONS=rps_sustained bench/gen-bench-data.sh   # 60s hold, published as `hono_sustained`
SUSTAIN_DURATION=30s SUSTAIN_REPS=3 SECTIONS=rps_sustained bench/gen-bench-data.sh
```

The two runs use the same Hono server and the same load generator, so the only
difference between them is fixed-count against fixed-window. A runtime that gives
back more than a few percent is one whose steady state is not its headline.

## Memory safety

`bench/memory-safety.sh` is not a speed benchmark. It runs three scripts that
each ask for more than the machine can give — a 200k-deep nested array through
`JSON.stringify`, a string doubled past the engine's maximum, and 10M chained
`.then()` — and records only *how* the runtime refuses.

`graceful` means JS got a catchable error or the process exited cleanly;
`crash:N` means it took signal N and the guest never got a say, which in a
server is the difference between one failed request and a dropped process.

```sh
bench/memory-safety.sh              # human-readable table
BENCH_JSON=1 bench/memory-safety.sh # the `memory_safety` section
```

It previously invoked `esrun <path-to-esrun>` and looked for LLRT at `../llrt`,
so neither ever ran, and it reached the site not at all. Both are fixed; it now
shares run.sh's runtime detection.

## Publishing to the site

`website/src/benchmarks.js` is generated, never edited. `bench/gen-bench-data.sh`
runs the four scripts in machine mode, merges their JSON, and writes the module:

```sh
bench/gen-bench-data.sh                       # everything
bench/gen-bench-data.sh url encoding          # re-measure rows, merge into the rest
SECTIONS=rps_static bench/gen-bench-data.sh   # one section only
SECTIONS="workloads memory_safety" ...        # or several
```

The module is fed by five independent scripts and re-running all of them takes
most of an hour, so `SECTIONS` picks which actually run: `workloads` (run.sh,
which owns every charted row), `rps` (Hono req/s), `rps_sustained` (the same
server held under load for a window), `rps_static` (64 KiB static file req/s),
`websocket` (the chat fan-out sweep), `http2`, and `memory_safety`.
A section left out keeps the values already in the module. `workloads` is the
one exception to merging — it owns the row matrices outright and replaces them,
so a row deleted from the suite does not live on in the data forever.

The point of a generated module is that no number on the site is ever typed by
hand — which only holds if a run that went wrong is **rejected** rather than
written out. A half-failed suite would otherwise produce a module full of nulls,
every chart would render "n/a", and the repair would be a human editing the
generated file. So `bench/validate-bench-data.mjs` gates the write:

- Every row the benchmarks page charts must exist and have at least one measured
  runtime, and every row the run publishes must reach the page. Both directions,
  because a group the page names but the run does not define renders an empty
  section, and a row measured but charted nowhere is a result quietly dropped.
  (The home page shows a subset by design — it is a shop window. The benchmarks
  page is the full table, and that is what this enforces.)
- Every value must be a finite non-negative number or `null`.
- The sections owned by the other three scripts (`results_rps`, `websocket`,
  `results_http2`) must be present and populated.
- Timeouts, cells under the measurement floor, and cells with a coefficient of
  variation above 10% are reported as warnings, so whoever publishes sees them
  and not just whoever happened to be watching stderr.
- **A minimum nothing corroborates is rejected outright.** The gate is
  `results_floor_gap` — how far the second-lowest sample sits above the lowest —
  and not CoV, because the published number is a *minimum*. One writeback stall
  sends CoV past 100% without moving the minimum a millisecond, so gating on
  spread rejects sound data: on `fsappend_large`, node measured CoV 67% with its
  floor corroborated to 1.9%, which is a perfectly good number. What is not good
  is a lone low sample nothing else comes near — bun once published a floor 668%
  below its own next-lowest reading. Above 25% gap the generation fails.

  That row is also why the gate exists. It appended 2 MB x 20, growing the file
  to 42 MB per launch and ~1 GB across a row, so past the kernel's dirty-page
  threshold the number tracked writeback rather than the runtime — and it was
  charted anyway at 168% variance. It is now 256 KB x 60, sized between the
  writeback threshold above and the measurement floor below.

On rejection the previous, known-good module is left in place and the generator
exits non-zero. The fix is to re-run the benchmark, never to edit the module.

Charts read the generated data directly and render nothing where it is missing.
They must not carry inline fallback numbers: the homepage req/s chart used to
have a `||` fallback that had gone stale by ~43% for esrun, and it would have
appeared the moment a run failed to produce the key — silently replacing a
measurement with a flattering guess.

## Caveats

- These are **microbenchmarks** — they isolate one thing each and don't predict
  whole-application performance.
- esrun runs **single-file classic scripts** (no ES-module loader) and grants all
  capabilities — it's a convenience runner, not a sandbox here.
- The crypto shapes reflect esrun's **op model** (sync ops wrapped in promises)
  as much as the underlying libraries.
- `fetch` hits a trivial local server returning 64 bytes — it measures the
  request/response *plumbing* and the provider seam, not throughput or TLS.
