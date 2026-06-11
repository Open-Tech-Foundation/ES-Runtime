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
> - **Primitive-only value marshaling.** `engine::Value` marshals JS primitives;
>   every other value collapses to `Value::Other(String(value))`. *Reason:*
>   structured + zero-copy marshaling belongs with the op system and the perf
>   pass (Phases 2/8). *Impact:* callers cannot yet inspect object/array
>   structure through the boundary.
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
> - **`DOMException` is not yet a real JS class.** Bare V8 has no `DOMException`
>   constructor; it is a web-platform class defined only once the runtime prelude
>   exists (Phase 4). Until then a `DOMException`-classed error (e.g. a capability
>   denial → `NotAllowedError`) is thrown as a plain `Error` whose message is
>   prefixed with the `.name`. *Impact:* JS sees the right text but not yet the
>   right class/`instanceof`.
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

### D9 — Crypto backend · *Open*
**Context:** `crypto.subtle` must use vetted, constant-time primitives; never hand-rolled.
**Options:** `ring` (fast, audited, narrower algorithm set, build-toolchain constraints) vs **RustCrypto** suite (pure-Rust, broad coverage, easier portability) — or a hybrid.
**Decision:** _Pending maintainer sign-off._ Default lean: RustCrypto for breadth + portability unless a perf gap dictates `ring`.
**Consequences:** Determines algorithm coverage, build complexity, and cross-platform story.

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
**Consequences:** Clean Layer-B seam and a real second-engine boundary now; `runtime` gains a `providers` dependency in Phase 3. See D3a (Phase 2) for the boundary leak notes.
**Deferred (not silently):** **Panic-across-FFI containment** for op/timer/reject callbacks (`catch_unwind` per D12) is implemented in the **hardening phase (§6.9)**, alongside the heap/watchdog/stack limits — not in Phase 2. Until then a *host-written* op handler that panics aborts the process (Rust's `extern "C"` panic = abort, not UB); hostile **JS** cannot force this, since handlers validate their marshaled arguments and return typed `OpError`s. Tracked for Phase 9.
