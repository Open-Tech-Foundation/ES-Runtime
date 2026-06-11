# Changelog

All notable changes to ES-Runtime are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the project is
pre-`0.1.0` and the public API is unstable.

## [Unreleased]

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

### Phase 6 will add

Fetch family (SPEC.md §6.6): `Headers`, `Request`, `Response`, the `Body` mixin,
and `fetch` over a new `NetTransport` provider (streaming bodies via the Phase 5
streams), plus `Blob`, `File`, `FormData`.
