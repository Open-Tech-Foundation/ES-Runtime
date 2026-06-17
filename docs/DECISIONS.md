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
> - ~~**Uncaught-exception JS class not preserved.**~~ *Resolved (Phase 8):* Exception classes (including `DOMException` names) and JS stack traces (`at fn (file:line:col)`) are now preserved and surfaced through `engine::Error` into the CLI.
> - **Primitive-only value marshaling.** `engine::Value` marshals JS primitives
>   plus, since Phase 6, `Value::Bytes` (`Uint8Array`/typed-array views, **copied**
>   to/from `Vec<u8>`). Every other value still collapses to
>   `Value::Other(String(value))`. *Reason:* structured marshaling belongs with
>   later phases; **zero-copy** `ArrayBuffer` transfer (avoiding the `Value::Bytes`
>   copy) was **audited in Phase 8 — see that note below**. *Impact:* byte bodies
>   cross the boundary correctly but with a copy; objects/arrays still don't.
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
> - **`DOMException` is fully real.** The prelude defines a real
>   `globalThis.DOMException` class, so prelude APIs (atob/btoa,
>   structuredClone, Abort) throw the correct type with `instanceof Error`.
>   Errors thrown natively from the **engine** (Rust side, e.g. a capability
>   denial → `NotAllowedError`) now correctly resolve this class from
>   `globalThis` during construction, surfacing as true `DOMException` instances
>   to JS.
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
>
> **Phase 8:**
> - **Startup-snapshot baking does not cross the boundary.** `runtime` drives
>   snapshot creation through `V8Engine::build_snapshot`/`with_snapshot`, naming
>   the *engine* type (already re-exported) but no V8 type. The op-id ordering
>   contract (build and restore must register ops in the same order) is a real
>   coupling, documented on both methods. *Impact:* the snapshot feature is
>   engine-specific by nature; a second engine would expose its own equivalent.
> - **Zero-copy `ArrayBuffer` transfer — audited, deliberately deferred.** Two
>   `Value::Bytes` copy points exist: **(in)** `convert::marshal` copies a
>   typed-array view into a fresh `Vec` via `copy_contents`; **(out)**
>   `bytes_to_uint8array` does `bytes.to_vec()` before
>   `ArrayBuffer::new_backing_store_from_vec` (the backing-store adoption itself
>   is *already* zero-copy). *Audit outcome:*
>   - The **in** copy is the load-bearing one and the hard one to remove: true
>     zero-copy would hand Rust a borrow into a V8 backing store, but ops —
>     especially async ones polled on a later `tick` — outlive the call scope,
>     and V8 may move, detach, or free the buffer in between. Sound zero-copy
>     would need an externalize/detach protocol over pinned backing stores; the
>     unsafe surface and lifetime risk outweigh the win for now.
>   - The **out** copy is merely an artifact of `value_to_js(&Value)` borrowing;
>     an owned-`Value` return path could move the `Vec` straight into the backing
>     store. A low-risk follow-up, not taken here to avoid reshaping the
>     marshaling ownership model mid-phase.
>   *Decision (Phase 8):* keep copying — both paths are correct and bounded by
>   body size — and revisit alongside the broader structured-marshaling work.

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
**Consequences:** Broad coverage with a pure-Rust, cross-platform build; per-algorithm crates to track. The TLS backend (D20, rustls) is independent of this choice. **Scope:** Phase 7 shipped getRandomValues, randomUUID, and `subtle` digest/HMAC/AES-GCM. Phase 7b completes `crypto.subtle`: AES-CBC/CTR, HKDF/PBKDF2 derivation, ECDSA/ECDH over P-256/384/521 (`p256`/`p384`/`p521`), and RSA — RSASSA-PKCS1-v1_5/RSA-PSS/RSA-OAEP (`rsa`). The one carried gap is the `rsa` Marvin advisory (see the RSA note below + `SECURITY.md`).

**7b dependency note (maintainer-approved, 2026-06-12):** The symmetric/KDF crates (`aes`/`cbc`/`ctr`, `hkdf`/`pbkdf2`) reuse the existing `aes` 0.8 / `hmac` 0.13 / `sha2` 0.11 with no duplicates. The **EC** crates, however, sit on the older `elliptic-curve` 0.13 / `digest` 0.10 generation — the unifying `elliptic-curve` 0.14 is still pre-release — so they pull **duplicate `digest` 0.10, `sha2` 0.10, and `hkdf` 0.12** into the tree (warn-level under `deny.toml`'s `multiple-versions = "warn"`; accepted as the cost of stable EC, to be revisited when 0.14 ships). To keep arbitrary `algorithm.hash` working, ECDSA computes the message prehash with our `sha2` 0.11 and calls `sign_prehash`; **JWK is assembled in JS** so the curves' `jwk` feature stays off. ECDSA signing routes its nonce through the **Entropy provider** (`RandomizedPrehashSigner`), never ambient `OsRng` — important because P-521's deterministic path reaches for `OsRng` directly. EC keys cross the op boundary as PKCS#8 (private) / SEC1 points (public).

**RSA backend + accepted advisory (maintainer, 2026-06-12):** RSA (RSASSA-PKCS1-v1_5, RSA-PSS, RSA-OAEP) uses the RustCrypto **`rsa`** crate (0.9), staying within D9's RustCrypto lock and reusing the EC generation's `digest` 0.10 (only `sha1` 0.10 — `sha1_rsa`, for the SHA-1 DigestInfo OID — is added, since `rsa` re-exports `sha2` but not `sha1`). All RSA randomness (key gen, PSS salt, PKCS#1 blinding, OAEP padding) routes through the **Entropy provider**. Keys cross the op boundary as PKCS#8 (private) / SPKI (public); JWK (incl. private CRT params) is assembled in JS via a small length-prefixed framing. **Limitation:** `rsa` 0.9's OAEP label is `&str`, so non-UTF-8 labels are rejected (SPEC §7). **Accepted security gap:** the `rsa` crate carries **RUSTSEC-2023-0071** (Marvin timing sidechannel, medium, *no fix available*). Alternatives were weighed and rejected — **aws-lc-rs** is constant-time but forfeits Entropy-provider-routed randomness (ambient OS CSPRNG) and adds a C/asm backend to `runtime`; **openssl-rs** adds a system dependency (regressing SPEC §1/D2). Accepted because RSA private-key ops are host-side (no guest timing oracle) and there is no pure-Rust constant-time RSA today. Tracked explicitly in `deny.toml` + `.cargo/audit.toml` and on the **revisit list in `SECURITY.md`** (revisit when RustCrypto ships constant-time RSA or the stack moves to the `digest` 0.11 generation).

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

**Resolved (Phase 9, 2026-06-12):** The V8-invoked callbacks (`op_dispatch`, `timer_set`, `timer_clear`, `promise_reject_callback`) now run inside `catch_unwind`. A panic in a host op handler or in marshaling is contained as a JS exception (`"internal error in host op"`) instead of unwinding across V8's C++ frames. Containment assumes `panic = "unwind"` (the default); under `panic = "abort"` the process aborts, which is then the chosen policy. Landed with the execution watchdog, near-heap-limit guard, bounded pending-ops, and the V8-native stack guard (see SPEC §4 / CHANGELOG Phase 9).

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
**Consequences:** Well-tested, ~WHATWG-conformant parsing for low effort; `runtime` gains a `url` dependency (MIT OR Apache-2.0). Minor WHATWG gaps are tracked against WPT (D13) — note: the `hostname`/`host` setter port handling gap was resolved. **URLPattern** is not covered by the crate and is instead implemented via a custom, efficient JavaScript compilation strategy in the runtime prelude.

---

### D19 — Streams: hand-written, default-first · *Locked (maintainer sign-off, 2026-06-12)*
**Context:** The WHATWG Streams spec is the largest min-common item. The choices were full vs. default-first scope, and hand-writing vs. vendoring the standards reference JS.
**Decision (maintainer):** **Hand-write** the spec's abstract operations in the prelude (no external code — fits the from-scratch ethos of D2; no large vendored blob or extra license/attribution), and **default-first**: ship `ReadableStream` (default), `WritableStream`, `TransformStream`, both queuing strategies, backpressure, `tee`, `pipeTo`/`pipeThrough`, and the encoding streams now. **Byte/BYOB streams** (`ReadableByteStreamController`, BYOB readers) are deferred to a follow-up sub-phase (SPEC §7).
**Consequences:** Full control and a clean dependency graph; more implementation care, with conformance tracked against WPT (D13). Streams live in one prelude IIFE (`streams.js`) so the interdependent readable/writable/transform/pipe machinery can share internal slots (a module-private `Symbol`); encoding streams build on the public `TransformStream`.

**Update (Phase 9, 2026-06-12):** **Byte/BYOB streams shipped** — `type: "bytes"` `ReadableStream` + `ReadableByteStreamController`, `ReadableStreamBYOBReader`/`Request`, `autoAllocateChunkSize`, the pull-into queue, and `byobRequest.respond`/`respondWithNewView`, hand-written to the spec's abstract operations. One deliberate deviation: enqueued chunks are **copied** into controller-owned buffers and BYOB views are filled in place — we do **not** transfer/detach ArrayBuffers (single-threaded, so the spec's detach dance is unnecessary; true zero-copy is the D3a follow-up).

---

### D20 — Fetch: vetted HTTP client (reqwest + rustls), confined to default-providers · *Locked (maintainer sign-off, 2026-06-12)*
**Context:** `fetch` needs an HTTP client for the default `NetTransport`. A from-scratch HTTP/1.1 client was considered (fits D2), but HTTP framing is security-sensitive and **TLS cannot be hand-rolled** without violating §7/D9 ("vetted, constant-time crypto only; no hand-rolled primitives").
**Decision (maintainer):** Use a **vetted HTTP crate** — `reqwest` with **rustls** TLS (no OpenSSL/native-tls), HTTP/1.1 + HTTP/2 — for the default transport, **confined to `default-providers`**. `runtime`/`engine`/`providers` never depend on it; the seam is the `NetTransport` trait. Streaming model for Phase 6: **buffered request body, streamed response body** (response chunks pulled on tick into a JS `ReadableStream`); streaming request bodies are a follow-up (SPEC §7).
**Consequences:** Battle-tested framing/TLS for low risk; a large dependency tree, but isolated to the batteries crate (the audit/deny surface grows only there). `deny.toml` adds `CDLA-Permissive-2.0` for the rustls root-cert bundle. The TLS crypto backend (rustls' provider) is **not** the `crypto.subtle` backend — **D9** stays *Open* until Phase 7. A new engine `Value::Bytes` variant (copying `Uint8Array` ↔ `Vec<u8>`) carries byte bodies across the op boundary; zero-copy is Phase 8 (D3a).

### D21 — ES modules: async pre-load + sync resolve, engine owns the graph, loader is a capability-checked provider · *Locked (maintainer sign-off, 2026-06-13)*
**Context:** SPEC §1 calls for executing ES modules, not just classic scripts. V8's module-resolution callback is **synchronous** — when a module instantiates, V8 demands an already-compiled module for each import then and there — yet reading a module's source is I/O, which the runtime performs only through async provider traits (D5). The module graph is also deeply V8-coupled (`v8::Module`, scopes, resolve/`import.meta` callbacks), which must not leak into `runtime` (D3).
**Decision (maintainer):**
- **Three phases, split across the boundary.** `runtime` runs an **async load phase** that walks the graph (compile → read `module_requests` → resolve + load each dependency → compile → recurse), deduping by canonical specifier so diamonds and cycles load once; then `engine` does **synchronous instantiation** (its resolve callback is a pure lookup over the fully-compiled id map the load phase built) and **evaluation** (a promise; top-level await settled by the driven loop).
- **`engine` owns the graph behind an opaque `ModuleId`** (a `u32` newtype). New `Engine` methods — `compile_module`, `module_requests`, `instantiate_module`, `evaluate_module`, `module_eval_state` — name no V8 type; the registry, resolve callback, and `import.meta.url` initializer live in `engine`, reached via an isolate slot (callbacks are `UnitType` and cannot capture).
- **Loading is a capability-checked provider.** A new `ModuleLoader` trait (`resolve` pure, `load` async) is the seam; `default-providers` ships `FsModuleLoader` (canonical ids are `file://` URLs — so `import.meta.url` is a URL and relatives resolve with WHATWG semantics) and a deny-all default. `runtime` gates *following an import* on **`Capability::FileSystem`**; a self-contained module (no imports) needs no capability since its entry source is supplied by the caller.
- **Local files only for v1.** Relative/absolute-path/`file:` specifiers; **bare specifiers and remote (`http:`) schemes are rejected** (consistent with the "no npm/CommonJS/`node:`" non-goal, SPEC §125). Dynamic `import()` was deferred here and is now implemented (D23); import attributes / JSON modules and remote modules remain deferred (SPEC §7).
- **`esrun` runs every input as a module.** Modules give top-level await natively, so the old async-IIFE wrapper is dropped. This is a deliberate **backward-incompatible** change: inputs now run in module scope (strict mode, `this === undefined` at top level, no implicit global creation). `-e` snippets get a synthetic `file://<cwd>/[eval]` id so their relative imports resolve against the working directory.
**Consequences:** Correct, spec-shaped module loading with a clean boundary (a second engine could implement the same `ModuleId` methods). No new external references, so the build-time prelude snapshot (D8) is unaffected. `FsModuleLoader` does **not** yet confine resolution to a root (a path may escape via `..`/symlinks) — root-jailing is a hardening follow-up tracked with the FS provider. Module **semantics** (default/namespace/re-export/live-bindings/eval-order/cycles/dependency errors + TLA) are covered by the `runtime` test suite and `runtime-cli` end-to-end tests rather than the curated conformance suite, whose harness evaluates classic scripts.

### D22 — node_modules resolution for ES module packages (narrows the no-npm non-goal) · *Locked (maintainer sign-off, 2026-06-13)*
**Context:** D21 rejected bare specifiers, citing the "no npm/CommonJS/`node:`" non-goal (SPEC §125). Real programs nonetheless want to `import` installed dependencies. The maintainer chose to **narrow** that non-goal to allow resolving an *existing* `node_modules` tree — without becoming a Node-compatible runtime.
**Decision (maintainer):**
- **ESM packages only.** Resolve bare specifiers and load packages that ship ES modules; a CommonJS package is **rejected with a clear, package-named message** (no `require`/`module.exports` shim — that would reverse the no-CommonJS stance wholesale). `node:` builtins (and their bare spellings) are rejected. Nothing is installed — we resolve what is already on disk (`npm install` is the user's job).
- **Minimal resolution.** Walk `node_modules` upward from the referrer; read `package.json`; resolve the subpath via `exports` (string form, the `import`/`default` conditions of the object form, exact subpath keys, and **subpath patterns** — a single-`*` key like `"./fn/*"` with the captured portion substituted into the target, longest-prefix wins) or, with no `exports`, `module` → `main` → `index`; probe the target for `.js`/`.mjs`/`.cjs` + `index.*`. ESM-ness is decided by extension + package `"type"`. **Excluded:** full condition precedence (`node`/`browser`/nested beyond `import`/`default`), `"imports"`/`#internal`, self-reference. Resolution reads `package.json`, so **`ModuleLoader::resolve` is async** (it was made async ahead of this; a pure path loader just returns a ready future).
- **Where it lives.** A new `NodeModuleLoader` in `default-providers` (the only crate doing real I/O); the strict `FsModuleLoader` (relative/`file:` only, no `node_modules`) is kept for embedders who want no package resolution. `runtime`/`engine` are unchanged — this is purely a loader. `serde_json` parses `package.json` (already present transitively, so no new crate; confined to `default-providers`, like reqwest under D20). `esrun` uses `NodeModuleLoader` (a superset — it still handles relative/`file:`).
**Consequences:** Most modern, pure-ESM packages load with no extra ceremony; the large CommonJS tail does not, by design, but fails legibly rather than mysteriously. The `node_modules` walk widens filesystem reach, so the **no-root-confinement caveat** (D21 / SECURITY-REVIEW) matters more — an embedder sandboxing untrusted code should supply a root-confining `ModuleLoader` and withhold `FileSystem`. Resolution correctness is covered by `NodeModuleLoader` unit + integration tests (temp `node_modules` trees) and a `runtime-cli` end-to-end test. SPEC §125 is amended accordingly; CommonJS, `node:` builtins, remote modules, and bare-specifier *patterns* remain out.

### D23 — Dynamic `import()` (lifts the D21/D22 deferral) · *Locked (maintainer sign-off, 2026-06-13)*
**Context:** D21 deferred dynamic `import()`. Unlike static imports — whose whole graph is loaded *before* instantiation — `import()` is raised *during* evaluation and must return a promise that resolves with the module namespace once the imported module (and any top-level await in it) has fully evaluated. ES also requires a single module map per realm, so a module imported both statically and dynamically is one instance.
**Decision (maintainer):**
- **Engine** installs V8's host-import-module-dynamically callback (isolate config, snapshot-safe like the import-meta initializer). The synchronous callback records `(specifier, referrer)` and returns a `PromiseResolver`'s promise; the runtime drains the requests, loads + instantiates the graph, then `link`s the module (kicks off `Module::evaluate`, which is idempotent) and, once that evaluation settles, resolves the request with the namespace or rejects with the error. Everything crosses the boundary as ids/strings — no V8 type (D3).
- **Runtime** gains a **persistent realm module map** (canonical specifier → `ModuleId`) shared by static load and `import()`, and **stores the loader** (`Arc<dyn ModuleLoader>`) so dynamic imports raised mid-execution can reach it. `process_dynamic_imports()` is an async drive step (resolution/loading is I/O); `tick()` settles linked imports whose evaluation completed; `has_pending_work()` covers in-flight ones.
- **Driver** calls `process_dynamic_imports()` each loop iteration — no Driver API change; the runtime owns the loader.
**Consequences:** `import()` works for everything the static loader supports (local files + `node_modules` ESM), shares instances with static imports, and resolves correctly after a top-level-await dependency completes. The driven, no-owned-loop model is preserved: dynamic loading happens in the executor-driven async step, not inside the synchronous tick. Still deferred: import attributes / JSON modules, remote modules, and the `node_modules` extras from D22.

### D24 — v1 productionization direction: single repo, dual target; ESM-only; `runtime:` std modules; FS root jail; versioning from 0.1.0 · *Locked (maintainer sign-off, 2026-06-13)*
**Context:** Hardening toward a production v1 used **both** as an embeddable library and as a strengthened standalone runtime. The maintainer set firm, non-reversible direction.
**Decision (maintainer):**
- **No Layer A/B split in practice.** This one repo is stabilized for embedding *and* strengthened standalone; there is no separate runtime to defer serving/I/O to. The provider seam stays (an embedder can still inject), but the standalone runtime grows real host capabilities here.
- **ESM-only, permanently.** No CommonJS interop, ever (no `require`/`module.exports` shim). Bare specifiers resolve `node_modules` for ESM packages only; CJS packages are rejected with a clear message. (Supersedes any future temptation to revisit D22's CJS stance.)
- **Host capabilities are `runtime:` standard modules, not globals.** New scheme `runtime:<name>` exposes host I/O as **async ES modules** the guest imports explicitly — `runtime:fs`, `runtime:net`, `runtime:http`, `runtime:process` (e.g. `import { readFile } from "runtime:fs"`, `import { env, args } from "runtime:process"`). All filesystem ops are async. These names are reserved as part of the versioned API. Each module is capability-gated.
- **Filesystem root confinement by default.** Module/file resolution is jailed to a root (default: the working directory); `..`/symlink escapes are rejected. A CLI flag relaxes/extends it (e.g. additional allowed roots, or disable). "We are not going back" on jail-by-default.
- **API versioning from 0.1.0.** Semver starts now; the public Rust API and the `runtime:` namespace are the versioned contract (workspace version bumped 0.0.0 → 0.1.0).
- **Cross-platform CI.** Add Windows to CI next, macOS after (no macOS hardware on hand yet).
**Consequences:** A coherent productionization path. The `runtime:` modules need new provider traits (`FileSystem`, a process/env provider, an HTTP-server/listener provider) and capability-gated ops, served as baked module sources via the loader — a multi-part effort planned separately. ESM-only narrows ecosystem reach (the CJS tail of npm stays out) — an accepted, deliberate product boundary. Root-jail-by-default may require a flag for legitimate cross-root setups; that flag is the sanctioned escape hatch.

### D25 — FS sandbox: realpath resolution (pnpm-correct) + project-root jail · *Locked (maintainer sign-off, 2026-06-13)*
**Context:** Root-jailing module/file resolution (D24) must not break pnpm. Investigation found the loader did **not** resolve pnpm transitive deps even *before* any jail: pnpm puts a package's own deps as symlinked peers inside `node_modules/.pnpm/<pkg>@<ver>/node_modules/`, reachable only by following the package symlink to its real location. The loader kept the *logical* symlink path, so the upward `node_modules` walk never found them. (This corrects an earlier "lexical, don't realpath" idea — empirically wrong.)
**Decision (maintainer):**
- **Realpath resolved modules** (Node default, `--preserve-symlinks` off): canonicalize a module's resolved path so its referrer — and thus its dependency walk — starts from the *real* location. This makes pnpm's `.pnpm` store resolve transitively; `import.meta.url` then shows the real path (as Node does). The entry is already canonicalized by the CLI.
- **Project-root jail, by default.** Confine every resolved (real) path under a **root** = the nearest ancestor of the entry/base directory containing `node_modules` or `package.json` (else the base directory itself). The `node_modules` walk stops at the root. An escape (a `..` or a symlink whose realpath leaves the root) is rejected.
- **Why this is pnpm-safe:** for a standard single-project pnpm install, `.pnpm` lives under `<project>/node_modules`, so every realpath stays under the project root → resolves *and* passes the jail. Monorepo/workspace symlinks (realpath above a sub-package), global `pnpm link`, or a symlinked external store can resolve outside the root — those need the relax flag (additional allowed roots), which is D24's deferred CLI part.
- **esrun** sets the loader's base to the **entry file's directory** (cwd for `-e`) so root detection finds the project around the script being run, not the process cwd.
**Consequences:** pnpm (and npm/yarn) single-project installs work, including transitive deps; resolution is sandboxed to the project by default with no flag. `import.meta.url` reflects realpaths (`.pnpm/...`), matching Node. Workspaces/global-links await the relax flag. Applies to `NodeModuleLoader` (the CLI loader); the strict `FsModuleLoader` is unchanged for now.

### D26 — `runtime:` built-in module scheme + `runtime:process` · *Locked (maintainer sign-off, 2026-06-14)*
**Context:** First implementation of the `runtime:` standard-module namespace reserved in D24. `runtime:process` was chosen as the first module (over `runtime:path`) because it is more practical on its own (env/args) and it *unblocks* `runtime:path` — whose `resolve`/platform-awareness need cwd/platform from process. It also exercises the op+provider wiring the later modules (`fs`/`net`/`http`) need, so it's a better foundation than pure-JS `path`.
**Decision (maintainer):**
- **Mechanism.** `runtime:<name>` is intercepted in the runtime's own graph walk + dynamic `import()`, *before* the injected `ModuleLoader`, and served from a baked source registry in the `runtime` crate (an ES module compiled through the normal pipeline, deduped via the realm module map, `import.meta.url = runtime:<name>`). Built-ins thus exist regardless of which loader (or none) an embedder installs and never touch the filesystem. The capability check is in the **ops** (the security boundary, D7), not the module — importing the module succeeds; its ops throw without the capability.
- **`runtime:process` API** (named exports + a default aggregate), backed by a new capability-checked `Process` provider (`default-providers`' `SystemProcess` reads the real process; the CLI supplies the user `args`; an embedder injects a controlled view — no ambient authority, D5):
  - `env` — a **mutable in-process object** seeded from the host snapshot (writes/deletes work in-process; they do not yet propagate to the host or child processes — that lands with `child_process`).
  - `args` — frozen array of the user args (binary + script/`-e` excluded).
  - `cwd()` — function → working directory string.
  - `platform` / `arch` — **OS-native** values (`std::env::consts::OS` / `ARCH`: e.g. `"linux"` / `"x86_64"`).
  - `exit(code = 0)` — records the code on the provider and halts via the engine interrupt; the CLI reads the code and exits cleanly (not as an error).
  - Gated on a new **`Capability::Env`** (deny-by-default; `esrun` grants it).
- **Standards.** Aligned *in spirit* with the WinterTC CLI-API proposal (clean `args`, `process.env`-like `env`, `exit(code?=0)`), but the proposal is an unstable WIP global-`CLI` draft, so v1 binds to our `runtime:` module scheme instead; a thin `CLI` global re-exporting `runtime:process` is a cheap future add. `cwd`/`platform` are our supersets (not in the proposal); terminal metadata (interactive/NO_COLOR) is deferred to Phase 13.
**Consequences:** A reusable built-in-module mechanism and the first practical standard module. `runtime:path` follows, using `cwd()`/`platform`. The `Process` provider's `env` snapshot is read-only-to-host for now; host/child propagation is future work tied to `child_process`.

### D27 — Documentation process: every public API in repo MD + marketing site + API reference · *Adopted (maintainer sign-off, 2026-06-14)*
**Context:** As the `runtime:` surface grows (D26 onward), API docs must not drift. The maintainer set a standing rule: every new public/host API is documented in three places, kept in sync, as part of the same change that ships it.
**Decision (maintainer):**
- **Repo MD (canonical):** `docs/API.md` is the source-of-truth API reference in the repo — every `runtime:` module and its exports, with signatures, capability, and behavior notes. README's quick-start links to it.
- **Marketing site:** the `@opentf/web` app under `site/` (Vite + Tailwind v4, file-based routing in `site/app/`). Built with the org's own framework, **not React** (D-note). Public-facing overview + concept pages.
- **API reference (site):** `site/app/docs/**` mirrors `docs/API.md` as browsable reference pages (e.g. `site/app/docs/process/page.jsx` ↔ `runtime:process`).
- **Definition of done:** a PR that adds or changes a public API updates all three. The repo MD is authoritative if they ever disagree.
**Consequences:** Docs ship with the code. The site lives in-repo (`site/`, its own bun/Vite toolchain, `node_modules`/`dist` git-ignored) so it versions alongside the runtime. Bun is the JS package manager for `site/`.

### D28 — `runtime:net` TLS: scope vs the WinterTC Sockets API · *Locked (maintainer sign-off, 2026-06-16)*
**Context:** `runtime:net` ships plaintext TCP only; `connect({ secureTransport: "on" })` errors. Closing that gap means reconciling our surface with the [WinterTC Sockets API proposal](https://sockets-api.proposal.wintertc.org/), which is wider than a plain "wrap the stream in TLS." The proposal's `SocketOptions` is `{ secureTransport: "off"|"on"|"starttls", allowHalfOpen, sni, alpn }` and `SocketInfo` is `{ remoteAddress, localAddress, alpn }`, with a `Socket.upgraded` flag and a `startTls()` upgrade path. Our current wiring (`net_connect(host, port, tls: bool)`, a `SocketInfo` split into address/port with no `alpn`, no `sni`/`alpn`/`upgraded`) cannot express TLS faithfully, because real TLS pulls in SNI and ALPN. TLS must not be hand-rolled (§7/D9); the vetted rustls backend chosen in D20 for `fetch` is reused.

**Decision (maintainer):** Implement `secureTransport: "on"` to spec, in phased tranches confined to `default-providers` plus the thin plumbing the options require:
- **Bind now.** TLS client `connect` with certificate verification on by default; **SNI** (`sni` option, default = connect hostname); **ALPN** (offer `options.alpn`, surface the negotiated protocol as `SocketInfo.alpn`, `null` when none/plaintext); add `Socket.upgraded` (constant `false` until `startTls`).
- **Plumbing.** Replace the `NetProvider::connect` `tls: bool` parameter with a `ConnectOptions { secure, sni, alpn }` struct (future-proofs `starttls`); add `alpn: Option<String>` to `SocketInfo`; thread both through `net_connect` + `socket_json` + `net.js`. The capability boundary is unchanged — `connect` stays gated on `Capability::Net` (D7).
- **Crypto.** rustls via `tokio-rustls`, **`aws-lc-rs` provider** selected explicitly (both `ring` and `aws-lc-rs` are compiled in the tree, so `ClientConfig::builder()` would panic on an ambiguous default), trust anchors from **`webpki-roots`** (bundled Mozilla set — deterministic, no platform I/O; consistent with D20's "rustls, no OpenSSL"). Both crates are already in the lockfile via reqwest. Confined to `default-providers`; `deny.toml` already allows the cert-bundle license (`CDLA-Permissive-2.0`, D20).

**Deferred (each its own follow-up):**
- **`startTls()` / `secureTransport: "starttls"`** — our socket is split into reader/writer tasks over channels the instant it connects, so the raw stream cannot be reclaimed for an in-place upgrade. Supporting it needs `spawn_socket` restructured to defer task-spawn until after a possible upgrade, plus a `net_start_tls` op returning a new socket. `startTls()` keeps throwing meanwhile.
- **Server-side TLS** (`listen` termination) — not in the `listen()` surface; belongs to the `runtime:http` server story (needs cert/key loading + a capability decision).
- **`allowHalfOpen`** and the **combined `"host:port"` `SocketInfo` shape** — pre-existing WinterTC divergences, unrelated to TLS; tracked separately.

**Consequences:** A spec-faithful TLS client (SNI + ALPN) at low risk, behind the existing `NetProvider` seam. The trait signature changes once (`ConnectOptions`), absorbing `starttls` later without another break. No new capability; no engine/runtime dependency on rustls (stays in `default-providers`).
