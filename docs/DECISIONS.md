# DECISIONS

Architecture decision record for the embeddable JavaScript runtime (**Layer A**). Each entry: context, decision, consequences. Append new decisions; never silently rewrite a locked one — supersede it with a new entry.

Status: **Locked** · **Proposed** · **Open** (needs maintainer sign-off) · **Superseded**.

---

### D1 — Implementation language: Rust · *Locked*
**Context:** Need memory safety, zero-cost FFI to V8, strong async story, and a from-scratch systems build.
**Decision:** Rust (stable, pinned MSRV). `#![forbid(unsafe_op_in_unsafe_fn)]`; `unsafe` isolated and documented.
**Consequences:** Excellent control and safety; V8 binding requires careful FFI discipline.

---

### D2 — V8 via the raw `v8` crate; no runtime framework · *Locked*
**Context:** Need V8's JIT and mature isolate model. Existing frameworks (`deno_core`, etc.) would shortcut the work but couple us to their op/loop/extension model.
**Decision:** Use only the low-level **`v8` crate** (FFI bindings). Build the embedding, op system, module loader, event loop, and snapshotting **from scratch**. **No `deno_core` / `deno_runtime` / any runtime framework.**
**Consequences:** Full control over the integration seam (critical for Layer B); more upfront work; we own all embedding correctness.

---

### D3 — Engine abstraction boundary · *Locked*
**Context:** Want to add a second engine (e.g. JavaScriptCore) later without rewriting the API layer.
**Decision:** All `v8`-crate usage confined to the `engine` crate behind an abstraction (lifecycle, execution control, value marshaling, op registration, module instantiation, snapshots). `runtime` depends only on the abstraction and **never names a V8 type**.
**Consequences:** Swappable engines; but full hiding of V8 is hard — handle/scope/value semantics leak. Leak points are documented per-occurrence (see D3a placeholder below). Success test: a second engine slots in without editing `runtime`.

> **D3a — Engine boundary leak points** · *Open (living list)*
> Record here each place the V8 abstraction is necessarily leaky, with the reason, as they arise during implementation.
>
> **Phase 1:**
> - **No engine *trait* yet — concrete `Engine` only.** The abstraction in §3 is
>   currently a concrete `engine::Engine` whose public surface already names no
>   V8 type (inputs/outputs are `std`/`common` types). A formal trait is
>   deferred to Phase 2, when the op system gives a second consumer to design it
>   against; extracting it then must not change the public types. *Reason:*
>   avoid speculative abstraction before there is a second implementor/consumer.
> - **Uncaught-exception JS class not preserved.** `engine::Error::Execution`
>   carries only the stringified exception message, so it maps to a generic JS
>   `Error` rather than the original subclass (`TypeError`, etc.). *Reason:*
>   reconstructing the class requires reading the thrown object's constructor and
>   re-mapping; deferred to Phase 2 when ops re-enter JS. *Impact:* lossy error
>   class on the JS round-trip.
> - **Primitive-only value marshaling.** `engine::Value` marshals JS primitives
>   plus, since Phase 6, `Value::Bytes` (`Uint8Array`/typed-array views, **copied**
>   to/from `Vec<u8>`). Every other value still collapses to
>   `Value::Other(String(value))`. *Reason:* structured marshaling belongs with
>   later phases; **zero-copy** `ArrayBuffer` transfer (avoiding the `Value::Bytes`
>   copy) is the Phase 8 perf pass (ARCHITECTURE §9). *Impact:* byte bodies cross
>   the boundary correctly but with a copy; objects/arrays still don't.
> - **Snapshot-creation concurrency constraint leaks to the caller.** V8 forbids
>   building a snapshot concurrently with other isolate creation; `snapshot::build`
>   documents this as a caller obligation rather than hiding it. *Reason:*
>   inherent V8 global-state limitation. *Impact:* embedders must build snapshots
>   before spawning isolates (natural at startup); tests serialize via a guard.
>
> **Phase 2:**
> - **Engine trait now extracted** (resolving the Phase 1 "concrete only" note).
>   `engine::Engine` is object-safe and names no V8 type; `runtime` holds a
>   `Box<dyn Engine>`. The boundary held — no V8 type appears in `runtime`.
> - **`DOMException` is partially real (updated Phase 4).** The prelude now
>   defines a real `globalThis.DOMException` class, so prelude APIs (atob/btoa,
>   structuredClone, Abort) throw the correct type with `instanceof Error`. The
>   remaining gap: errors thrown from the **engine** (Rust side, e.g. a capability
>   denial → `NotAllowedError`) still surface as a plain `Error` with a
>   name-prefixed message, because the engine has no handle to the JS class. A
>   later phase reconciles the two paths (engine throws the prelude's
>   `DOMException`).
> - **Async readiness is observed only on `tick`.** With no reactor (std-only,
>   `Waker::noop`), a pending op's future is polled when the embedder ticks, not
>   when its work actually becomes ready. *Reason:* the driven model (D4); a real
>   reactor arrives with tokio default-providers (Phase 3). *Impact:* completion
>   latency is bounded by tick cadence, which the embedder controls.
> - **Timer JS callbacks stay in `engine`.** A JS function can't cross the
>   boundary as a marshaled `Value`, so `setTimeout` callbacks are held in
>   `engine` and invoked by id (`fire_timer`); `runtime` owns only the schedule.
>   *Reason:* keeps `runtime` free of V8 handles. *Impact:* timer wiring is split
>   across the boundary by design.

---

### D4 — Driven event loop; runtime owns no loop or thread · *Locked*
**Context:** Layer B's scheduler must own execution timing. A self-running loop would have to be ripped out at integration.
**Decision:** Expose a tick/poll API advancing timers → ready async ops → microtask checkpoint → unhandled rejections. The embedder drives it. `runtime` spawns no thread and no loop.
**Consequences:** Clean Layer-B integration; standalone use requires `default-providers` to drive ticking (on tokio).

---

### D5 — All I/O injectable via provider traits; no ambient authority · *Locked*
**Context:** The runtime must not hold ambient access to network/clock/entropy/FS; capabilities must be embedder-controlled, and runs must be reproducible.
**Decision:** Define provider traits (`Clock`, `Entropy`, `Timers`, `NetTransport`, `FileSystem`, `TaskSpawner`) in `providers/`; concrete impls only in `default-providers/`. `runtime` makes no direct OS calls for time/entropy/network/FS.
**Consequences:** Reproducible under deterministic providers; clean seam for Layer B; a little extra indirection.

---

### D6 — Compatibility target: WinterTC Minimum Common Web API (2025 snapshot) · *Locked*
**Context:** "Full Node" was explicitly dropped. Need a standards-backed, tractable surface that's portable across runtimes.
**Decision:** Target the Ecma TC55 (WinterTC) Minimum Common Web API as the baseline. **No Node API / npm / CommonJS.**
**Consequences:** Far smaller, standardized surface; portable code; not drop-in for Node-targeted libraries.

---

### D7 — Deny-by-default capabilities + runtime-enforced resource limits · *Locked*
**Context:** Executed JS may be adversarial; the host must be protected.
**Decision:** Every side-effecting op is capability-gated. Runtime enforces: per-isolate heap limit (near-heap-limit callback → graceful kill), CPU/time watchdog (interrupt + `TerminateExecution`), stack-depth guard, bounded pending-op concurrency. No Rust panic crosses FFI.
**Consequences:** Hostile-input-grade containment; some performance overhead on the boundary; predictable failure modes.

---

### D8 — Pure-JS APIs shipped via a baked V8 startup snapshot · *Locked*
**Context:** Many min-common APIs are pure JS; re-evaluating prelude per context is slow and matters for Layer B density.
**Decision:** Bake the JS prelude into a V8 startup snapshot; only world-touching behavior is an op.
**Consequences:** Fast context creation; snapshot build step in the toolchain; prelude must be snapshot-safe.

---

### D9 — Crypto backend: RustCrypto · *Locked (maintainer sign-off, 2026-06-12)*
**Context:** `crypto.subtle` must use vetted, constant-time primitives; never hand-rolled (§7).
**Options weighed:** `ring` (fast, audited, narrower — no AES-CBC/RSA-OAEP/P-521) vs the **RustCrypto** suite (pure-Rust, broad WebCrypto coverage, portable) vs a hybrid.
**Decision (maintainer):** **RustCrypto**, for breadth + portability (the original default lean). Phase 7 uses `sha1`, `sha2`, `hmac`, and `aes-gcm`; the remaining algorithm crates (`ecdsa`/`p256`/`p384`, RSA) are added with Phase 7b. Crypto runs in `runtime` ops (it is computation, not I/O); `random_bytes` draws from the `Entropy` provider.
**Consequences:** Broad coverage with a pure-Rust, cross-platform build; per-algorithm crates to track. The TLS backend (D20, rustls) is independent of this choice. **Scope:** Phase 7 ships getRandomValues, randomUUID, and `subtle` digest/HMAC/AES-GCM; ECDSA/ECDH/RSA are staged (SPEC §7).

---

### D10 — License: Apache-2.0 · *Locked*
**Context:** OpenTF project; other OpenTF deliverables have used AGPL-3.0, so AGPL-3.0 was the original default lean. The repository, however, already ships an `Apache-2.0` `LICENSE` + `NOTICE` ("Copyright 2026 Open Tech Foundation and its contributors"), committed by the maintainer.
**Decision (2026-06-11):** **Apache-2.0**, confirmed by the maintainer, superseding the earlier AGPL-3.0 lean. Every crate sets `license = "Apache-2.0"`; the `cargo-deny` license allowlist (`deny.toml`) is built around an Apache-2.0-compatible permissive set.
**Consequences:** Permissive downstream embedding/distribution terms, including for Layer B. A patent grant + `NOTICE` attribution apply per Apache-2.0.

---

### D11 — Workspace / crate layout · *Locked*
**Context:** Need enforced dependency direction and a clean I/O seam.
**Decision:** Crates `common` → `engine` → `providers` → `runtime` → `default-providers` → `runtime-cli`, with the dependency direction in `ARCHITECTURE.md` §2. `v8` only in `engine`; real I/O only in `default-providers`.
**Consequences:** Boundaries enforced by the crate graph, not convention.

---

### D12 — Error model · *Locked*
**Context:** Errors must map cleanly to JS and never destabilize the host.
**Decision:** One typed error enum per layer; mapped to JS exception classes (`TypeError`, `RangeError`, `DOMException`, …); FFI boundaries `catch_unwind`-wrapped; no error swallowed.
**Consequences:** Predictable, debuggable failures; some mapping boilerplate.

---

### D13 — Conformance via WPT / Minimum Common API suite · *Locked*
**Context:** Need objective correctness signal for web APIs.
**Decision:** Run the official Minimum Common Web API / relevant Web Platform Test subset per implemented API; track pass-rate in-repo.
**Consequences:** High-confidence correctness; test-harness integration effort.

---

### D14 — Generic, target-agnostic · *Locked*
**Context:** No specific workload chosen; usage to emerge over time.
**Decision:** Build a general-purpose embeddable runtime; bake in no workload-specific assumptions.
**Consequences:** Broader applicability; some convenience features deferred until a real consumer (Layer B) defines them.

---

### D15 — Phase 2 op-system & loop structure · *Locked (maintainer sign-off, 2026-06-11)*
**Context:** The roadmap places the public tick/poll loop in `runtime` (§6.4) but the op system + driven loop is Phase 2 (§6.2), and `runtime` formally depends on `providers` (Phase 3). A decision was needed on where the Phase 2 machinery lives and how async ops run before tokio exists.
**Decision (with maintainer sign-off):**
1. **Introduce `runtime` now (Phase 2)**, depending on `engine` + `common` only (not yet `providers`); it owns the op wiring, the tick/poll loop, and the timer schedule.
2. **Extract the engine abstraction trait now** (`engine::Engine`; impl `engine::V8Engine`), resolving the Phase 1 "concrete only" note.
3. **Async ops are std-only, poll-on-tick** (`Waker::noop`, no reactor); tokio integration arrives with default-providers (Phase 3).
4. Ops are exposed at the low-level `globalThis.__ops.<name>`; ergonomic WinterTC wrappers come with the prelude (Phase 4/8). Timers (`setTimeout` &c.) are engine builtins; the embedder supplies time per tick until the `Clock`/`Timers` providers do (Phase 3).
**Consequences:** Clean Layer-B seam and a real second-engine boundary now. See D3a (Phase 2) for the boundary leak notes.
**Deferred (not silently):** **Panic-across-FFI containment** for op/timer/reject callbacks (`catch_unwind` per D12) is implemented in the **hardening phase (§6.9)**, alongside the heap/watchdog/stack limits — not in Phase 2. Until then a *host-written* op handler that panics aborts the process (Rust's `extern "C"` panic = abort, not UB); hostile **JS** cannot force this, since handlers validate their marshaled arguments and return typed `OpError`s. Tracked for Phase 9.

---

### D16 — Phase 3 provider integration: traits + driver, runtime API unchanged · *Locked (maintainer sign-off, 2026-06-11)*
**Context:** Phase 3 adds the provider traits and tokio defaults. The open question was how deeply to wire them into `runtime` now — internalize a `Clock` (pull time, drop `tick`'s `now_ms`) vs. keep the explicit driven API and let a driver supply time.
**Decision (with maintainer sign-off):** **Providers + driver only.** Define the traits in `providers`; implement tokio + deterministic test providers and a `Driver` in `default-providers`. `runtime`'s public API is **unchanged** (`tick(now_ms)` stays); the `Driver` reads the `Clock` and supplies the time. `runtime` gains its `providers` dependency only when it first consumes a provider-backed web API (`performance.now` → Phase 4, `getRandomValues` → Phase 7), per "add the dependency when it is used."
**Consequences:** Minimal, no boundary churn; the explicit `tick(now)` seam (D4) is preserved and the `Driver` is the swappable concrete loop. `getrandom` supplies raw OS entropy for the `Entropy` default — it does **not** resolve **D9** (the `crypto.subtle` algorithm backend), which stays *Open* until Phase 7.

---

### D17 — `console` is an injectable output-sink provider · *Locked (maintainer sign-off, 2026-06-11)*
**Context:** SPEC §2.2 requires console output to reach "the embedder's logging sink, not stdout." Options weighed: route straight to `tracing`; a `Console` provider trait; or an ad-hoc host callback.
**Decision (maintainer):** **Hybrid.** A minimal `Console` output-sink **provider** is the seam; the shipped default forwards to `tracing`. Rationale: `console.*` is the *guest program's* output, not the runtime's telemetry, so — like every other side effect — it is injected, not pulled from an ambient global. This (1) preserves no-ambient-authority (D5); (2) lets a hostile guest's output be bounded/rate-limited/dropped rather than flooding host telemetry (§7); (3) gives Layer B per-tenant sink isolation. `default-providers` ships `TracingConsole` (default), `NullConsole` (deniable), and `CapturingConsole` (tests).
**Consequences:** `Console` joins the provider list as the lightest provider (an output sink, no capability beyond "may emit"); `runtime` consumes it via `HostProviders`. The JS-facing `console` object is identical to a raw-tracing approach, so this is an internal seam, not a guest-visible commitment.

---

### D18 — URL family via the `url` crate · *Locked (maintainer sign-off, 2026-06-11)*
**Context:** WHATWG URL is large and subtle; a from-scratch JS implementation carries real conformance risk.
**Decision (maintainer):** Implement `URL`/`URLSearchParams` parsing and serialization with the servo **`url`** crate behind sync ops; the JS wrappers provide the surface. `URLSearchParams` is pure JS.
**Consequences:** Well-tested, ~WHATWG-conformant parsing for low effort; `runtime` gains a `url` dependency (MIT OR Apache-2.0). Minor WHATWG gaps (e.g. the `hostname` setter's port handling) are tracked against WPT (D13). **URLPattern** is **not** covered by the crate and is deferred (SPEC §7).

---

### D19 — Streams: hand-written, default-first · *Locked (maintainer sign-off, 2026-06-12)*
**Context:** The WHATWG Streams spec is the largest min-common item. The choices were full vs. default-first scope, and hand-writing vs. vendoring the standards reference JS.
**Decision (maintainer):** **Hand-write** the spec's abstract operations in the prelude (no external code — fits the from-scratch ethos of D2; no large vendored blob or extra license/attribution), and **default-first**: ship `ReadableStream` (default), `WritableStream`, `TransformStream`, both queuing strategies, backpressure, `tee`, `pipeTo`/`pipeThrough`, and the encoding streams now. **Byte/BYOB streams** (`ReadableByteStreamController`, BYOB readers) are deferred to a follow-up sub-phase (SPEC §7).
**Consequences:** Full control and a clean dependency graph; more implementation care, with conformance tracked against WPT (D13). Streams live in one prelude IIFE (`streams.js`) so the interdependent readable/writable/transform/pipe machinery can share internal slots (a module-private `Symbol`); encoding streams build on the public `TransformStream`.

---

### D20 — Fetch: vetted HTTP client (reqwest + rustls), confined to default-providers · *Locked (maintainer sign-off, 2026-06-12)*
**Context:** `fetch` needs an HTTP client for the default `NetTransport`. A from-scratch HTTP/1.1 client was considered (fits D2), but HTTP framing is security-sensitive and **TLS cannot be hand-rolled** without violating §7/D9 ("vetted, constant-time crypto only; no hand-rolled primitives").
**Decision (maintainer):** Use a **vetted HTTP crate** — `reqwest` with **rustls** TLS (no OpenSSL/native-tls), HTTP/1.1 + HTTP/2 — for the default transport, **confined to `default-providers`**. `runtime`/`engine`/`providers` never depend on it; the seam is the `NetTransport` trait. Streaming model for Phase 6: **buffered request body, streamed response body** (response chunks pulled on tick into a JS `ReadableStream`); streaming request bodies are a follow-up (SPEC §7).
**Consequences:** Battle-tested framing/TLS for low risk; a large dependency tree, but isolated to the batteries crate (the audit/deny surface grows only there). `deny.toml` adds `CDLA-Permissive-2.0` for the rustls root-cert bundle. The TLS crypto backend (rustls' provider) is **not** the `crypto.subtle` backend — **D9** stays *Open* until Phase 7. A new engine `Value::Bytes` variant (copying `Uint8Array` ↔ `Vec<u8>`) carries byte bodies across the op boundary; zero-copy is Phase 8 (D3a).
