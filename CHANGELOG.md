# Changelog

All notable changes to ES-Runtime are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the project is
pre-`0.1.0` and the public API is unstable.

## [Unreleased]

### Added

- **`@opentf/esrun-types`** — hand-written TypeScript definitions for the
  `runtime:` standard modules (`runtime:process`, `runtime:path`, `runtime:fs`),
  in [`types/`](types/), for editor completion and type-checking. Ambient
  `declare module` blocks; add via `tsconfig` `types` or a triple-slash
  reference. Validated with `tsc --strict`. Also emitted by **`esrun types`**
  (`esrun types > esrun.d.ts`, Deno-style) and shipped under `types/` in the
  release archive — the definitions are baked into the binary as a static
  string, so they add nothing to startup or runtime cost.
- **Benchmarks** — split the file-I/O workload into **read / write / append**
  and added a **glob scan** workload (Deno has no built-in runtime glob → n/a),
  all cross-runtime; numbers regenerated on the benchmarks page.

- **`runtime:fs`** — modern, Blob-based file I/O, the third `runtime:` standard
  module (SPEC §11, DECISIONS D25). `file(path)` is a lazy, Blob-like handle
  (`text`/`json`/`bytes`/`arrayBuffer`/`stream`/`exists`/`stat`/`write`/`delete`,
  plus `writable()` — a web-standard `WritableStream` for piped/incremental
  writes). `write(dest, body, { append })` takes any web body
  (string/Blob/ArrayBuffer/TypedArray/Response/ReadableStream/`file()`). Plus
  `readDir`, `stat`, `exists`, `mkdir`, `remove`, `rename`, and a `Glob`
  (`match` pure/sync, `scan` async over the jailed walk; `globset`/`walkdir`,
  `**`/`{a,b}` semantics). All operations are **async** (no sync variants, no
  callbacks). Backed by a new injectable `FileSystem` provider (tokio
  `SystemFileSystem`) and ops gated on new `Capability::FileRead` /
  `Capability::FileWrite`; every path is confined to the project **root jail**
  (D25 — `..`/symlink escapes rejected). New `examples/modules/fs.mjs`.

- **`runtime:path`** — modern, platform-aware path utilities, the second
  `runtime:` standard module (DECISIONS D26, SPEC §11). A pure-computation ES
  module that takes the host platform and `cwd()` from `runtime:process` (so it
  carries `Env`); separators and `resolve()` follow the real OS. Exports `sep`,
  `delimiter`, `isAbsolute`, `normalize`, `join`, `resolve`, `dirname`,
  `basename`, `extname`, `parse`, `relative`, and `file:` URL interop
  (`fromFileURL`/`toFileURL` — `dirname(fromFileURL(import.meta.url))` is the
  modern `__dirname`). One platform-correct surface: no `posix`/`win32` dual
  namespaces and no overloaded signatures. New `examples/modules/path.mjs`.

## [0.1.0] - 2026-06-14

### Project

- **API versioning starts at 0.1.0** (was `0.0.0`). Semver from here; the public
  Rust API and the `runtime:` standard-module namespace are the versioned
  contract. Locked v1 direction in DECISIONS **D24**: single repo serving both
  embedding and standalone use; **ESM-only, permanently** (no CommonJS interop);
  host capabilities exposed as async `runtime:` modules (`runtime:fs`,
  `runtime:net`, `runtime:http`, `runtime:process`) rather than globals;
  filesystem **root confinement by default** (CLI opt-out); Windows CI next,
  macOS after.

### Added

- **`runtime:` standard modules + `runtime:process`** — a built-in module scheme:
  `runtime:<name>` is served by the runtime itself (loader-independent, never
  touches the filesystem), with the capability check in the ops. First module
  `runtime:process` exposes `env` (mutable in-process snapshot), `args` (user
  args), `cwd()`, `platform` + `arch` (OS-native, e.g. `"linux"`/`"x86_64"`), and
  `exit(code = 0)` (halts + sets the process exit code) — gated on a new
  `Capability::Env`, backed by a new `Process` provider (`SystemProcess` reads
  the real process; embedders inject a controlled view). Aligned in spirit with
  the WinterTC CLI-API proposal (DECISIONS D26). New `examples/modules/process.mjs`.
- **ES modules** — `esrun` now runs every input as an ES module: static
  `import`/`export`, `import.meta.url`, and native top-level `await`. Imports
  resolve as **local files** (relative/absolute paths or `file:` URLs) through a
  new capability-checked `ModuleLoader` provider; `default-providers` ships
  `FsModuleLoader` (file-backed) and a deny-all default. The engine gained
  module compile/instantiate/evaluate behind an opaque `ModuleId` (no V8 type
  crosses the boundary), and `runtime` gained an async graph loader
  (`Runtime::load_module_source`) that walks + dedups the import graph before
  V8's synchronous instantiation, then settles top-level await on the driven
  loop. Loading an import requires `Capability::FileSystem`; a self-contained
  module runs without it. **Backward-incompatible:** inputs now run in module
  scope (strict mode, `this === undefined`), and the old async-IIFE wrapper for
  top-level await is gone (modules provide it natively). Import attributes /
  JSON modules and remote modules are not yet supported (DECISIONS D21). New
  `examples/modules/`.
- **Dynamic `import()`** — `import(specifier)` resolves with the module
  namespace after the imported module (and any top-level await in it) fully
  evaluates, and shares instances with static imports via a realm module map.
  The engine installs V8's host-import callback and settles the request once
  evaluation completes; `runtime` stores the loader and exposes an async
  `process_dynamic_imports()` drive step the `Driver` calls each iteration.
  Works for everything the static loader supports (local files + `node_modules`
  ESM packages). DECISIONS D23. New `examples/modules/dynamic.mjs`.
- **`node_modules` resolution (ES module packages)** — bare specifiers
  (`import x from "pkg"`, `"pkg/sub"`, `"@scope/pkg"`) resolve against an
  existing `node_modules` tree via the new `NodeModuleLoader`: walk
  `node_modules` upward, read `package.json` (`exports` string + `import`/
  `default` conditions + subpath patterns like `"./fn/*"`, or
  `module`/`main`/`index`), probe `.js`/`.mjs`/`.cjs`.
  **ES module packages only** — CommonJS packages and `node:` builtins are
  rejected with a clear message; nothing is installed (run `npm install`
  yourself). This narrows the no-npm non-goal (SPEC §125 amended; DECISIONS
  D22). `ModuleLoader::resolve` is now **async** (resolution does I/O); the
  strict file-only `FsModuleLoader` is kept for embedders wanting no
  `node_modules`. Adds `serde_json` to `default-providers` (already present
  transitively — no new crate).

### Performance

- **Prelude snapshot baked into `esrun`** — `build.rs` now builds the V8 startup
  snapshot at compile time and `include_bytes!`s it into the binary; the CLI
  restores it via `Runtime::with_snapshot` instead of compiling + evaluating the
  ~16 prelude files on every launch. Startup drops to ~6.6 ms (fastest of
  node/bun/deno/esrun on the bench box). Host-arch builds only — cross-compiling
  the CLI would need a target-run step (noted in `build.rs`).
- **Op-backed `atob`/`btoa`** — base64 transcoding moves from a pure-JS
  per-character concatenation into host `base64_encode`/`base64_decode` ops
  (`base64_ops.rs`); ~4.5× faster on the base64 workload (386 → 86 ms). Same
  semantics, including forgiving-base64 decode (with one recorded looseness:
  all trailing `=` are stripped).
- **URL ops return offsets, not JSON** — `url_parse`/`url_set` now return the
  canonical href plus 15 component offsets (`url::Position`s, UTF-16 indices)
  as one small JS array (new `Value::Array`); every `URL` getter is a lazy
  `href.slice(...)` and `.origin` is a separate lazy op. Replaces the 11-field
  JSON round-trip (~3× faster URL workload in `bench/`); same shape Node's Ada
  integration uses. Wire format documented in `url_ops.rs`/`url.js`.
- **Zero-copy op returns** — op results are consumed, not cloned: a returned
  `Value::Bytes` vec moves into the `ArrayBuffer` backing store (was: two extra
  copies per `TextEncoder.encode`), `utf8_encode` reuses the marshaled string's
  buffer, and `utf8_decode` converts valid UTF-8 in place. The JS→Rust crossing
  still copies (zero-copy there remains D3a/Phase 8).
- **Lazy HTTP client** — `ReqwestTransport` builds its reqwest client (TLS
  config, root store) on first `fetch` instead of at construction. Startup
  drops ~15 ms → ~8.5 ms (fastest of node/bun/esrun on the bench box); scripts
  that never fetch never pay for the client.
- **Sub-millisecond `performance.now()`** — new defaulted
  `Clock::monotonic_micros` (overridden by `SystemClock`); `performance.now()`
  now has µs precision instead of integer ms. Deterministic/test clocks are
  unaffected (default derives from `monotonic_ms`).
- **Release profile** — `lto = "thin"` + `codegen-units = 1` for the Rust-side
  hot paths (V8 is prebuilt; unaffected). `panic = "abort"` deliberately not
  set (D15 containment relies on unwinding).

### Fixed

- **`console.log` object inspection** — replaced the `JSON.stringify`-based
  formatter (which silently dropped function-valued properties, `undefined`,
  symbols, etc. — so an object/module-namespace full of functions printed as
  `{}`) with a recursive `util.inspect`-lite: functions as `[Function: name]` /
  `[class Name]`, arrays/objects/Map/Set/Error/RegExp/Date, null-prototype and
  module-namespace objects, nested quoting, a depth limit, and a circular guard.
  (A namespace import of a function-only package such as `moderndash` now prints
  its members instead of `{}`.)
- **`TextEncoder.encodeInto`** — `read`/`written` are now spec-correct under
  truncation: output is cut at a UTF-8 code-point boundary (never mid-sequence)
  and `read` counts only the UTF-16 code units actually encoded (was: always
  reported the full source length).

### Benchmark

- `bench/` reworked and broadened from 4 workloads to 15 plus a peak-RSS row.
  The `webapi` workload is split into `url` and `encoding` (separately
  attributable); new workloads add a pure-engine `json` baseline, large-document
  `jsonbig`, key-based `crypto` (HMAC + AES-GCM), `base64`, `structured`
  (`structuredClone`), `async` (microtask overhead), `timers`, `streams`,
  `fetch` (against a local server — the first workload to exercise the network
  provider seam), and `bigscript` (user-source parse cost). Deno is now detected
  (incl. `~/.deno/bin/deno`); workloads run an untimed JIT warmup and report the
  **median** of `WORKLOAD_RUNS` (default 5); `BENCH_JSON=1` emits machine-
  readable output and `WORKLOADS=...` runs a subset. Representative results
  refreshed across all four runtimes.

### Performance (earlier in this cycle)

- **Op-backed `TextEncoder`/`TextDecoder`** — UTF-8 transcoding now rides V8's
  native UTF-16↔UTF-8 conversion via `utf8_encode`/`utf8_decode` ops instead of
  a pure-JS code-point loop. ~47% faster on encode+decode; behaviour unchanged
  (fatal/BOM/replacement still correct). (Investigated structured marshaling for
  the URL path — returning a built JS object instead of JSON — and **reverted
  it**: per-property Rust→V8 object construction is slower than V8's native
  `JSON.parse`. Noted in `bench/README.md`.)
- **Lazy `URLSearchParams`** — `new URL()` no longer parses the query into a
  `URLSearchParams` eagerly; it's built on first `.searchParams` access. Cuts
  ~38% off URL construction for the common case that never reads `.searchParams`
  (no behaviour change; setters resync only a materialized instance). Measured
  in the cross-runtime benchmark (`bench/`).

### Tooling — standalone `esrun` CLI + crate rename

- **`esrun`** (`es-runtime-cli`) — a standalone binary that wires the default
  tokio providers and runs a JavaScript file or `-e <code>` snippet end-to-end
  (the §8 standalone embedding). Grants all capabilities (trusted-local-script
  mode). Inputs run as ES modules (see **Added** above), with native top-level
  `await`. **Single self-contained binary** — V8 is statically linked and the
  prelude is embedded; no asset directory. Example scripts under `examples/`;
  `cargo build-cli` builds it; `cargo install --path crates/runtime-cli` puts
  `esrun` on `PATH`.
- **Crate rename:** the flagship library crate `es-runtime-runtime` → **`es-runtime`**
  (import `es_runtime`); directory stays `crates/runtime`.

### Phase 9 (in progress) — Hardening: the safety spine

The resource-limit and FFI-safety guarantees (SPEC.md §4) that demonstrably stop
a runaway or heap-bomb script without harming the host. Fuzzing, sanitizer CI,
WPT conformance, and byte/BYOB streams remain for later Phase 9 passes.

#### Added

- **Execution watchdog** — `engine` exposes a thread-safe `InterruptHandle`
  (`terminate`/`is_terminating`; names no V8 type, so it stays within the engine
  boundary D3) and `Engine::interrupt_handle()`. `eval` detects a
  watchdog/heap termination and returns `Error::Terminated { reason }` rather
  than hanging; the engine recovers (the terminating state is cleared).
- **Near-heap-limit guard** — terminates execution and grants unwind headroom,
  so a heap bomb surfaces as `Terminated("heap limit exceeded")` instead of an
  OOM crash.
- **Bounded pending-ops** — `OpState` enforces `Limits::max_pending_ops`; the
  over-limit async dispatch throws a `RangeError`.
- **Panic-across-FFI containment (resolves D15)** — the V8-invoked callbacks
  (`op_dispatch`, `timer_set`, `timer_clear`, `promise_reject_callback`) run
  inside `catch_unwind`; a Rust panic in a host op handler or in marshaling is
  contained as a JS exception, never an unwind across V8's C++ frames (assumes
  `panic = "unwind"`).
- **Stack guard** — documented + tested: V8's native guard turns unbounded
  recursion into a catchable `RangeError`.
- **`esrun -t/--timeout <ms>`** — a watchdog thread terminates the engine after
  the deadline (cross-thread V8 termination stops even a synchronous infinite
  loop), with a tokio-timeout backstop for async-callback runaways. `Runtime`
  exposes `interrupt_handle()`.

- **Internal security review + docs finalization** — `docs/SECURITY-REVIEW.md`:
  a consolidated threat model, trust boundaries, attack-surface→defense table,
  and a residual-risk register (fuzzing/external-review pending, `rsa` advisory,
  SES deferral, `panic=abort` caveat, watchdog scope). Finalized SPEC §8
  definition-of-done status, refreshed ARCHITECTURE §7/§9 (intrinsic integrity,
  snapshot done / zero-copy deferred), and cross-linked from `SECURITY.md`.
- **Intrinsic-integrity audit** (§4) — confirmed + documented that the security
  boundary is in Rust: the op table and capability set live in `OpState`, so
  guest JS tampering (prototype pollution, global reassignment, forging
  `__ops`) can't escalate privilege or dispatch an ungated op. Added `harden.js`
  (last prelude fragment) as defense-in-depth: locks the `globalThis.__ops`
  binding (object stays extensible for op registration) and freezes `console`.
  3 tamper-resistance tests. SES-style primordial freezing is deliberately
  deferred to the embedder/Layer B (SECURITY.md), not baked into Layer A.
- **Byte/BYOB streams** (§2.8, closes the §7 deferral) — `ReadableStream`
  `type: "bytes"` + `ReadableByteStreamController`, `ReadableStreamBYOBReader`,
  `ReadableStreamBYOBRequest`, `autoAllocateChunkSize`, the pull-into queue, and
  `byobRequest.respond`/`respondWithNewView`, hand-written to the WHATWG abstract
  operations (DECISIONS D19). Copy-based: enqueued chunks are copied into
  controller-owned buffers and BYOB views filled in place — no ArrayBuffer
  transfer/detach (single-threaded; zero-copy is the D3a follow-up). 5 new
  conformance assertions (now 62/62).
- **Conformance suite + pass-rate tracking** (§5/§8) — a curated in-repo set of
  spec-behaviour assertions (`crates/runtime/conformance/*.js`: encoding, base64,
  URL, structuredClone, events, abort, crypto, streams, performance) run by the
  `conformance_suite_passes` test, which is a CI gate. Zero-failure + a
  non-regressing count are enforced; the snapshot (currently **57/57**) is
  recorded in `conformance/RESULTS.md`. An in-JS harness provides
  `test`/`assert*` (sync + async).

#### Tests

- Watchdog stops a `while(true){}` from another thread (engine recovers after);
  a heap bomb is terminated cleanly; a panicking op surfaces as a catchable JS
  `Error`; the pending-op bound rejects the over-limit call; deep recursion is a
  typed error. Verified end-to-end via `esrun -t`.

### Phase 8 — Startup snapshot + perf

Bakes the prelude and op shells into a V8 startup snapshot (SPEC.md §6.8,
DECISIONS.md D8), so constructing a runtime can skip compiling *and* running the
prelude.

#### Added

- **`V8Engine::build_snapshot(configure)`** — runs op registration + the prelude
  into a snapshot-creator isolate and serializes the heap — and
  **`V8Engine::with_snapshot_baked_ops`** to restore it. The native callbacks
  (`op_dispatch`, `timer_set`, `timer_clear`) are registered as one canonical
  **external-reference list** supplied at both build and restore (matched by
  index, so ASLR-safe across processes).
- **`Runtime::build_snapshot(providers)`** and **`Runtime::with_snapshot(blob,
  providers)`**: the restore path rebinds only the Rust op handlers (the JS
  `__ops.<name>` shells and the prelude are baked) in the same order
  `build_snapshot` used, and skips prelude evaluation entirely.
- A lightweight **`bench` example** (`default-providers`, std-only — no bench
  framework) measuring fresh vs snapshot startup and op-dispatch throughput.
  Indicative: ~**2.3× faster** runtime startup from a snapshot.

#### Changed / audited

- **Zero-copy `ArrayBuffer` transfer audited and deferred** (D3a Phase 8): the
  `Value::Bytes` in-copy (`copy_contents`) is unsafe to elide while async ops
  outlive the call scope; the out-copy (`bytes.to_vec()`) is a low-risk
  follow-up. Both kept as copies for now — correct and bounded by body size.
- Only the JS heap is serialized into the snapshot (context, `__ops.<name>`
  shells with their op-ids, prelude state); Rust handler closures are not.

### Phase 7b — WebCrypto (AES block modes, key derivation, elliptic curve, RSA)

Completes `crypto.subtle` (SPEC.md §6.7 / §2.10): the remaining symmetric
ciphers, the key-derivation functions, elliptic-curve ECDSA/ECDH, and RSA — all
RustCrypto (DECISIONS.md D9).

#### Added

- **AES-CBC** (`encrypt`/`decrypt`, PKCS#7 padding; 128/192/256-bit keys) and
  **AES-CTR** (`encrypt`/`decrypt`; 128/192/256-bit keys; 32/64/128-bit counter
  widths) on `crypto.subtle`, plus `generateKey`/`importKey` for both. One CTR
  op backs encrypt and decrypt (the mode is symmetric).
- **`deriveBits`/`deriveKey`** via **HKDF** (SHA-1/256/384/512) and **PBKDF2**
  (HMAC-SHA-1/256/384/512). KDF base keys import as non-extractable `raw` keys;
  `deriveKey` targets AES-* and HMAC derived keys.
- New ops `subtle_aes_cbc_encrypt`/`_decrypt`, `subtle_aes_ctr`, `subtle_hkdf`,
  and `subtle_pbkdf2`, backed by the `aes`/`cbc`/`ctr` and `hkdf`/`pbkdf2`
  RustCrypto crates. `aes`/`cbc`/`ctr` are pinned to the `cipher` 0.4 generation
  so they reuse the same `aes` 0.8 that `aes-gcm` already pulls (no duplicate
  `aes`; `aes-gcm` 0.11, which would unify onto `cipher` 0.5, is still an rc);
  `hkdf`/`pbkdf2` 0.13 reuse the existing `hmac` 0.13 + `sha2`.
- Tests add NIST SP 800-38A vectors (CBC F.2.1, CTR F.5.1), RFC 5869 (HKDF) and
  RFC 6070 (PBKDF2) known-answer vectors, round-trips, and a PBKDF2→AES-GCM
  `deriveKey` end-to-end.
- **ECDSA** (sign/verify) and **ECDH** (`deriveBits`/`deriveKey`) over **P-256,
  P-384, P-521** on `crypto.subtle`, with `generateKey` (key pairs) and
  `importKey`/`exportKey` for **all four formats** (`raw`/`spki`/`pkcs8`/`jwk`).
  ECDSA honours an arbitrary `algorithm.hash` (SHA-1/256/384/512). New
  `ec_ops` module + ops (`ec_generate_pkcs8`, `ec_public_point`,
  `ec_private_scalar`, `ec_import_pkcs8`, `ec_pkcs8_from_scalar`,
  `ec_import_spki`, `ec_export_spki`, `ecdsa_sign`, `ecdsa_verify`,
  `ecdh_derive`), backed by `p256`/`p384`/`p521`.
- EC keys cross the op boundary as PKCS#8 (private) / SEC1 points (public); JWK
  is assembled in JS from the exposed coordinates/scalar. **ECDSA signing draws
  its nonce from the `Entropy` provider** (hedged `RandomizedPrehashSigner`),
  never ambient `OsRng` — notable for P-521, whose deterministic path otherwise
  reaches for `OsRng`.
- The EC crates sit on the older `elliptic-curve` 0.13 / `digest` 0.10
  generation (0.14 is pre-release), so they bring **duplicate `digest` 0.10,
  `sha2` 0.10, and `hkdf` 0.12** — warn-level under `deny.toml`, accepted per
  DECISIONS.md D9.
- Tests cover ECDSA P-256 sign/verify (+ tamper) and P-521/SHA-512, a P-384
  export→import round-trip across **all four formats**, ECDH shared-secret
  agreement, and an ECDH→AES-GCM `deriveKey` between two parties.
- **RSA** — **RSASSA-PKCS1-v1_5** and **RSA-PSS** (sign/verify) and **RSA-OAEP**
  (encrypt/decrypt) on `crypto.subtle`, with `generateKey` (key pairs) and
  `importKey`/`exportKey` for **spki/pkcs8/jwk** (private JWK incl. the CRT
  params `d`/`p`/`q`/`dp`/`dq`/`qi`). Arbitrary `algorithm.hash`
  (SHA-1/256/384/512). New `rsa_ops` module + ops backed by the `rsa` crate;
  JWK components cross the boundary via a small length-prefixed framing.
- All RSA randomness (key gen, PSS salt, PKCS#1 blinding, OAEP padding) routes
  through the **Entropy provider**, never ambient `OsRng`. `rsa`/`num-bigint-dig`
  are built at `opt-level = 3` in the dev profile so test-suite key generation
  stays fast (~1.4 s vs ~33 s).
- **Accepted security gap:** the `rsa` crate carries **RUSTSEC-2023-0071**
  (Marvin timing sidechannel, medium, no fix available). Maintainer-accepted
  with rationale — RSA private-key ops are host-side, and the alternatives
  (aws-lc-rs: ambient RNG + C backend; openssl-rs: system dep) cost more than
  they buy. Listed explicitly in `deny.toml` + `.cargo/audit.toml`; tracked on
  the new **`SECURITY.md`** revisit list. RSA-OAEP labels are UTF-8 only (an
  `rsa` 0.9 API limitation).
- New `SECURITY.md` records the project's supply-chain posture and the accepted
  advisory gaps (RSA Marvin, `paste` unmaintained).
- Tests: one 2048-bit key reused across PKCS1-v1_5 + PSS sign/verify, OAEP
  round-trip (with and without a label), and SPKI/PKCS8/JWK export→import with
  cross-verification.

### Phase 7 — WebCrypto (first tranche)

`crypto` (SPEC.md §6.7 / §2.10), backed by vetted RustCrypto primitives
(DECISIONS.md D9). Resolves the open D9 crypto-backend decision.

#### Added

- **`crypto.getRandomValues`** (fills an integer TypedArray in place) and
  **`crypto.randomUUID`** (v4), drawing from the `Entropy` provider — now wired
  into `HostProviders` (the D16-anticipated point).
- **`crypto.subtle`** (first tranche): `digest` (SHA-1/256/384/512), **HMAC**
  (`generateKey`/`importKey`/`exportKey`/`sign`/constant-time `verify`), and
  **AES-GCM** (`generateKey`/`importKey`/`exportKey`/`encrypt`/`decrypt`, tag
  mismatch → `OperationError`). Plus the `CryptoKey` class.
- Crypto runs in synchronous `runtime` ops (RustCrypto: `sha1`, `sha2`, `hmac`,
  `aes-gcm`); the prelude `subtle` wraps each in a Promise.
- Tests use known-answer vectors (SHA-256("abc")), HMAC sign/verify (incl.
  tamper), and AES-GCM round-trip + tamper rejection.

#### Decisions

- **D9 locked: RustCrypto** (breadth + portability). ECDSA/ECDH and RSA are
  staged for **Phase 7b** (SPEC §7). The TLS backend (D20) is independent.

### Phase 6 — Fetch family

`fetch` and its surrounding types (SPEC.md §6.6 / §2.9), networking routed
exclusively through a new `NetTransport` provider; response bodies stream via the
Phase 5 streams.

#### Added

- **Engine `Value::Bytes`** — the marshaler now converts `Uint8Array`/typed-array
  views ↔ `Vec<u8>` (copying), so byte bodies can cross the op boundary. True
  zero-copy `ArrayBuffer` transfer remains Phase 8 (DECISIONS.md D3a).
- **`NetTransport` provider** (`providers`) — outbound HTTP for `fetch`:
  `HttpRequest` (buffered body) → `HttpResponse` (metadata + a streamed
  `ByteStream` body, via `futures-core`). Capability-gated on `Capability::Net`.
- **default-providers** — `ReqwestTransport` (reqwest + rustls TLS, no OpenSSL;
  HTTP/1.1 + HTTP/2; streamed response bodies) and a deterministic
  `MockTransport`/`MockResponse` (testing) so fetch is tested without network.
- **runtime fetch** — capability-gated `fetch` async op + a `fetch_body_read`
  op that streams the response body into a JS `ReadableStream`. `HostProviders`
  gains the net provider.
- **Prelude**: `Headers` (case-insensitive, combining), the `Body` mixin
  (`arrayBuffer`/`text`/`json`/`blob`/`bytes`/`body` stream), `Request`,
  `Response` (+ `Response.json`/`error`), and `fetch`; `Blob`, `File`, and
  `FormData` (multipart encoding).
- New dependencies: `reqwest` (rustls), `futures-core`/`futures-util`; `url`
  unchanged. `deny.toml` allows `CDLA-Permissive-2.0` (the rustls root-cert
  bundle).

#### Decisions

- **D20** locked: after weighing a from-scratch HTTP client, use a **vetted HTTP
  crate** (reqwest + rustls) for the default `NetTransport` — HTTP/1.1 framing
  and TLS are security-sensitive, and **TLS may not be hand-rolled** (§7/D9).
  Confined to `default-providers`. Streaming model: **buffered request body,
  streamed response** for Phase 6; streaming request bodies are a follow-up
  (SPEC §7).

### Phase 5 — Streams

The Streams surface (SPEC.md §6.5 / §2.8) — the largest correctness item —
hand-written to the WHATWG abstract operations (DECISIONS.md D19), pure JS in the
prelude.

#### Added

- **`ReadableStream`** (default) + `ReadableStreamDefaultController` +
  `ReadableStreamDefaultReader`: enqueue/read/close/error/cancel, start/pull/cancel
  algorithms, `desiredSize` backpressure, `tee`.
- **`WritableStream`** + controller + writer: write/close/abort with the full
  erroring/abort state machine, `ready`/`closed` promises, backpressure.
- **`TransformStream`** + controller: transform/flush with backpressure linking
  the writable and readable sides.
- **`pipeTo`** (with `preventClose`/`preventAbort`/`preventCancel` + `AbortSignal`)
  and **`pipeThrough`**.
- **`CountQueuingStrategy`**, **`ByteLengthQueuingStrategy`**.
- **`TextEncoderStream`** / **`TextDecoderStream`** (deferred from Phase 4) on
  `TransformStream`, handling surrogate pairs / multi-byte UTF-8 split across
  chunk boundaries.
- A test harness (`eval_async`) that drives async JS to completion via the tick
  microtask loop.

#### Decisions

- **D19** locked (maintainer sign-off): Streams are **hand-written to spec**
  (fits the from-scratch ethos, D2) and **default-first** — byte/BYOB streams
  (`ReadableByteStreamController`, BYOB readers) are deferred to a follow-up
  (SPEC §7). Conformance tracked vs WPT (D13).

### Phase 4 — Core web primitives

The WinterTC pure-JS surface (SPEC.md §6.4), shipped as a JS prelude over the op
system, with world-touching parts as host ops.

#### Added

- **Prelude harness** — `runtime` now installs host ops and evaluates a JS
  prelude at construction (`Runtime::new` takes [`HostProviders`] and returns
  `Result`). Per D8 the prelude is snapshot-baked in Phase 8; evaluated at
  startup until then.
- **Console** as an injectable output sink (DECISIONS.md D17): a `Console`
  provider trait (guest output, not telemetry — boundable/attributable per §7),
  with `TracingConsole` (default → `tracing`), `NullConsole` (deniable), and
  `CapturingConsole` (tests). `console.*` formats args and routes through it;
  group/table are minimal.
- **performance** — `performance.now()` / `timeOrigin`, backed by the `Clock`
  provider (the D16 point where `runtime` gains its `providers` dependency).
- **Globals** — `queueMicrotask`, `reportError`, `structuredClone` (deep clone of
  the standard cloneable types incl. cycles; `DataCloneError` otherwise), and the
  `self` alias.
- **DOMException** — a real JS class in the prelude (closes the JS-class half of
  the D3a note), used by atob/btoa, structuredClone, and Abort.
- **Encoding** — `TextEncoder`/`TextDecoder` (UTF-8, pure JS) and `atob`/`btoa`.
- **URL family** — `URL` + `URLSearchParams`, parsing/serialization via the
  servo `url` crate behind sync ops (DECISIONS.md D18), with `search`/`searchParams`
  kept in sync.
- **Events** — `Event`, `CustomEvent`, `EventTarget` (flat dispatch: once,
  passive, signal, capture flag, `preventDefault`).
- **Abort** — `AbortController`, `AbortSignal` incl. `AbortSignal.abort`,
  `AbortSignal.timeout` (timer-driven), and `AbortSignal.any`.
- New dependency: `url`, in `runtime`.

#### Decisions

- **D17** (Console = injectable output-sink provider; default forwards to
  tracing) and **D18** (URL via the `url` crate) locked. Deferrals (SPEC §7):
  `TextEncoderStream`/`TextDecoderStream` → Phase 5 (need Streams); `URLPattern`
  → later; full WHATWG-URL conformance gaps tracked vs WPT.

### Phase 3 — Provider traits + default tokio providers

The I/O integration seam (SPEC.md §6.3): provider traits, reference tokio-backed
implementations, deterministic test providers, and a standalone driver.

#### Added

- **`es-runtime-providers` crate** — trait definitions only, no impls, no
  `unsafe` (ARCHITECTURE.md §6, DECISIONS.md D5): `Clock` (monotonic + wall ms),
  `Entropy` (fill CSPRNG bytes), `Timers` (`sleep` future), `TaskSpawner`
  (offload blocking work). `ProviderError` maps to a JS exception via
  `IntoException`. (`NetTransport`/`FileSystem` arrive with their consuming APIs.)
- **`es-runtime-default-providers` crate** — the **only** crate owning a real
  loop/clock/entropy:
  - Production impls: `SystemClock` (std `Instant`/`SystemTime`), `OsEntropy`
    (`getrandom`), `TokioTimers` (tokio timer wheel), `TokioTaskSpawner` (tokio
    blocking pool).
  - `Driver` — runs a `Runtime` to quiescence on tokio: reads the `Clock` for
    each tick's time, parks on `Timers` between ticks, accumulates unhandled
    rejections. This is the concrete loop `runtime` deliberately does not own
    (D4); Layer B swaps it for its scheduler.
  - `testing` module — deterministic providers (`ManualClock`, `ManualTimers`
    that advance a linked clock, seeded non-crypto `SeededEntropy`,
    `InlineTaskSpawner`) for reproducible runs (D5). The driver integration test
    runs an async op + a timer to completion with zero real waiting.
- New dependencies: `tokio` (rt + time) and `getrandom`, confined to
  `default-providers`.

#### Decisions

- **Providers + driver only** (maintainer sign-off): Phase 3 does **not** change
  `runtime`'s public API. `runtime` keeps `tick(now_ms)` and gains a `providers`
  dependency only when a provider-backed web API lands (`performance.now` →
  Phase 4, `getRandomValues` → Phase 7). The `Driver` supplies tick time from the
  `Clock`. **D9 (crypto.subtle backend) remains open** — `getrandom` is raw OS
  entropy, not the algorithm backend.

### Phase 2 — Op system + driven loop

The JS↔Rust op bridge and the embedder-driven event loop (SPEC.md §6.2): sync +
async ops, promise resolution, a microtask checkpoint, the tick/poll API, and
timer plumbing.

#### Added

- **Engine abstraction trait.** Extracted `engine::Engine` (object-safe, names no
  V8 type) from the concrete type, now `engine::V8Engine` (DECISIONS.md D3). The
  trait is the surface `runtime` depends on — a second engine could be slotted in
  without editing `runtime`.
- **`es-runtime-runtime` crate** — the driven runtime, built on the engine trait,
  with **zero direct `v8` dependency** and no `unsafe`. Holds a `Box<dyn Engine>`,
  the op wiring, and the timer schedule.
  - `Runtime::tick(now_ms) -> TickStatus` advances one step in order — due
    **timers → ready async ops → microtask checkpoint → unhandled rejections** —
    and reports work remaining + the next deadline so the embedder can park. No
    loop or thread is owned (DECISIONS.md D4).
  - `Runtime::register_op`, `set_capabilities`, `eval`, `has_pending_work`.
- **Op system** (`engine::op`) — a single non-capturing dispatch callback keyed by
  op id, op table in an isolate slot via `Rc<RefCell<_>>`:
  - Sync and async ops; arguments marshaled and **validated as untrusted**;
    **capability-check-first** dispatch (denied → clean JS exception, never a
    partial effect — ARCHITECTURE.md §4, D7). Ops exposed as `globalThis.__ops.<name>`.
  - Async ops return a real `Promise`; std-only **poll-on-tick** (no reactor,
    `Waker::noop`) settles them, then the microtask checkpoint runs reactions.
  - Errors carry their JS exception class via `OpError`/`IntoException`.
- **Timers** — `setTimeout`/`setInterval`/`clearTimeout`/`clearInterval` builtins;
  the engine holds the JS callbacks, the runtime owns the deadline-ordered
  schedule. Time is embedder-supplied per tick (the `Clock`/`Timers` providers
  become that source in Phase 3).
- **Unhandled-rejection tracking** via the promise-reject callback; surfaced in
  `TickStatus.unhandled_rejections`.
- Explicit microtask policy so reactions run only at the checkpoint, never
  implicitly mid-eval.

#### Decisions

- `runtime` introduced now (Phase 2) rather than Phase 4, and the engine trait
  extracted now — both per maintainer sign-off. New D3a leak notes: DOMException
  is not yet a real JS class (surfaced as `Error` with a name-prefixed message);
  async readiness is observed only on `tick`; timer JS callbacks stay in `engine`.

### Phase 1 — Foundation

Workspace, error model, observability, CI, and a V8 engine that runs `1 + 1`
end-to-end with snapshot scaffolding (SPEC.md §6.1).

#### Added

- **Cargo workspace** (`resolver = "3"`, edition 2024, MSRV 1.95) with the
  dependency direction from ARCHITECTURE.md §2 enforced by the crate graph
  (DECISIONS.md D11). Phase 1 introduces the first two crates; the rest land in
  their own phases.
- **`es-runtime-common`** — cross-cutting primitives, no I/O, no `unsafe`
  (`#![forbid(unsafe_code)]`):
  - Error model (DECISIONS.md D12): `ExceptionClass` JS-exception taxonomy, the
    `IntoException` trait each layer implements, the `common`-layer `Error`, and
    a `Result` alias.
  - `CapabilitySet` / `Capability` — deny-by-default capability tokens
    (DECISIONS.md D7); the empty set is the default.
  - `Limits` — per-isolate resource ceilings (heap, stack depth, pending ops)
    with validation and builder setters.
  - `telemetry::init_tracing` — idempotent `tracing` subscriber install
    (ARCHITECTURE.md §8).
- **`es-runtime-engine`** — the only crate using the `v8` crate (DECISIONS.md
  D2/D3):
  - One-time V8 platform init; `Engine` owning an isolate + a persistent
    context.
  - `Engine::eval` compiles and runs source under a `TryCatch`, marshaling JS
    primitives to `Value` and mapping failures to typed `Compile` / `Execution`
    errors — no panic crosses the boundary.
  - `snapshot::build` / `Engine::with_snapshot` — startup-snapshot build/load
    scaffolding (DECISIONS.md D8), proven by a prelude-state round-trip test.
  - The isolate heap ceiling from `Limits` is installed on creation.
- **CI** (`.github/workflows/ci.yml`) — all gates from SPEC.md §5: `fmt`,
  `clippy -D warnings`, `test`, `cargo-deny`, `cargo-audit`, and an MSRV (1.95)
  build.
- **Supply-chain config** — `deny.toml` with an Apache-2.0-compatible permissive
  license allowlist; `rust-toolchain.toml`, `rustfmt.toml`. One documented
  advisory ignore: `RUSTSEC-2024-0436` (`paste` unmaintained — informational
  only, reaches us transitively through `v8`, no fix available).

#### Decisions

- **D10 — License: Apache-2.0** locked (superseding the earlier AGPL-3.0 lean),
  matching the `LICENSE`/`NOTICE` already in the repo.
- **D3a** leak points recorded for the engine boundary (see DECISIONS.md):
  uncaught-exception JS class not yet preserved; primitive-only value marshaling;
  snapshot-creation concurrency constraint.

### Next

- **Phase 7b** — the rest of `crypto.subtle`: AES-CBC/CTR, ECDSA/ECDH (P-256/384/521),
  RSA (PKCS1/PSS/OAEP), HKDF/PBKDF2.
- **Phase 8** — bake the prelude into a V8 startup snapshot (D8); zero-copy
  ArrayBuffer audit; benchmarks.
- **Phase 9** — hardening: heap/CPU/stack limits, the watchdog, panic-across-FFI
  containment (D15), byte/BYOB streams, fuzzing, WPT conformance run.
