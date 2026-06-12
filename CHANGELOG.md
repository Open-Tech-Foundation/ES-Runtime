# Changelog

All notable changes to ES-Runtime are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the project is
pre-`0.1.0` and the public API is unstable.

## [Unreleased]

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
