# SPEC

Scope, API surface, conformance, and acceptance criteria for the embeddable JavaScript runtime (**Layer A**). See `ARCHITECTURE.md` for structure and `DECISIONS.md` for rationale.

Status legend: ☐ not started · ◐ in progress · ☑ done · ⊘ deferred (with note).

---

## 1. Scope

A production-grade, security-hardened, **embeddable** JavaScript runtime that:

1. embeds **V8** (via the raw `v8` crate; no runtime framework) and executes ES modules + scripts;
2. implements the **WinterTC (Ecma TC55) Minimum Common Web API** — 2025 snapshot;
3. owns **no I/O**: every side effect is supplied by an injectable provider trait;
4. is **driven** (tick/poll), never owning a loop or thread;
5. keeps V8 behind an **engine abstraction** so a second engine can be added later.

Generic and target-agnostic — no assumptions about a specific workload (multi-tenant, FaaS, etc.).

---

## 2. WinterTC Minimum Common Web API surface

Implement to spec; track conformance against the official Minimum Common Web API test suite and relevant Web Platform Tests.

### 2.1 Globals & structure
- ◐ `globalThis` wiring (+ `self`) ☑, `queueMicrotask` ☑, `structuredClone` ☑ (standard cloneable types + cycles), `reportError` ◐ (minimal: routes to console.error; ErrorEvent dispatch later). *(Phase 4.)*

### 2.2 Console
- ◐ `console` (log/info/warn/error/debug ☑) → the injected `Console` sink, not stdout (DECISIONS D17). group/table minimal. *(Phase 4.)*

### 2.3 Encoding
- ☑ `TextEncoder`, `TextDecoder` (UTF-8), `atob`, `btoa` *(Phase 4)*; `TextEncoderStream`/`TextDecoderStream` *(Phase 5, on `TransformStream`)*.

### 2.4 URL
- ◐ `URL`, `URLSearchParams` ☑ (via the `url` crate, DECISIONS D18). `URLPattern` ⊘ → later. *(Phase 4.)*

### 2.5 Timers (provider-backed)
- ◐ `setTimeout`, `clearTimeout`, `setInterval`, `clearInterval`. Mechanism in place (Phase 2): engine builtins + runtime-owned schedule, embedder-supplied time. Provider-backing (`Clock`/`Timers`) lands in Phase 3.

### 2.6 Abort
- ☑ `AbortController`, `AbortSignal` (incl. `AbortSignal.timeout`, `AbortSignal.any`). *(Phase 4.)*

### 2.7 Events
- ☑ `Event`, `EventTarget`, `CustomEvent` (flat dispatch model). *(Phase 4.)*

### 2.8 Streams (largest correctness item)
- ◐ `ReadableStream` (default) ☑, `WritableStream` ☑, `TransformStream` ☑, **backpressure** ☑, `CountQueuingStrategy`/`ByteLengthQueuingStrategy` ☑, `tee`/`pipeTo`/`pipeThrough` ☑ *(Phase 5, hand-written — DECISIONS D19)*. **byte/BYOB** streams ⊘ → follow-up.

### 2.9 Fetch family
- ◐ `Headers`, `Request`, `Response`, `Body` mixin, `fetch` ☑ — networking exclusively via the `NetTransport` provider; **response** bodies stream via §2.8. Request-body streaming ⊘ → follow-up (buffered for now). *(Phase 6, DECISIONS D20.)*
- ☑ `Blob`, `File`, `FormData`. *(Phase 6.)*

### 2.10 WebCrypto
- ☑ `crypto.getRandomValues` (Entropy provider), `crypto.randomUUID`. *(Phase 7.)*
- ☑ `crypto.subtle`: digest (SHA-1/256/384/512), HMAC, AES-GCM, AES-CBC, AES-CTR, `deriveBits`/`deriveKey` via HKDF + PBKDF2, ECDSA + ECDH over P-256/P-384/P-521, and RSA (RSASSA-PKCS1-v1_5, RSA-PSS, RSA-OAEP) — spki/pkcs8/jwk key formats *(Phase 7/7b, RustCrypto — DECISIONS D9)*. RSA carries an accepted timing-sidechannel advisory (SECURITY.md); RSA-OAEP labels are UTF-8 only (§7).

### 2.11 Performance
- ☑ `performance.now()`, `performance.timeOrigin` (Clock provider). *(Phase 4; integer-ms precision, sub-ms later.)*

Anything intentionally deferred from the snapshot is listed in §7 with rationale.

---

## 3. I/O provider contracts

Traits the embedder must satisfy (defaults shipped in `default-providers`):

- ☑ `Clock` — wall + monotonic time. *(Phase 3: trait + `SystemClock`/`ManualClock`.)*
- ☑ `Entropy` — CSPRNG bytes. *(Phase 3: trait + `OsEntropy`/`SeededEntropy`.)*
- ☑ `Timers` — schedule/cancel. *(Phase 3: trait + `TokioTimers`/`ManualTimers`.)*
- ☑ `TaskSpawner` — offload blocking work. *(Phase 3: trait + `TokioTaskSpawner`/`InlineTaskSpawner`.)*
- ☑ `Console` — guest output sink (the lightest provider; DECISIONS D17). *(Phase 4: trait + `TracingConsole`/`NullConsole`/`CapturingConsole`.)*
- ☑ `NetTransport` — outbound HTTP for `fetch`. *(Phase 6: trait + `ReqwestTransport`/`MockTransport`; DECISIONS D20.)*
- ☐ `FileSystem` — capability-scoped, async, optional/deniable. *(Later.)*

All calls: async-friendly, cancellable, capability-checked, typed errors. No provider, no capability ⇒ clean JS exception.

---

## 4. Resource limits & security guarantees

- ☑ Per-isolate **heap limit** → near-limit guard terminates execution before the host OOMs (Phase 9: `add_near_heap_limit_callback` → `Error::Terminated`).
- ☑ **Execution-time watchdog** → a runaway script is terminated via a thread-safe `InterruptHandle`; surfaces as `Error::Terminated`, never a hang (Phase 9). CPU-cycle accounting (vs wall-clock) is not separately implemented.
- ☑ **Stack-depth** guard → V8-native; unbounded recursion is a catchable `RangeError`, not UB or a hang (Phase 9 test).
- ☑ **Bounded pending-op** concurrency → `max_pending_ops`; the over-limit async dispatch throws `RangeError` (Phase 9).
- ☑ **Deny-by-default** capabilities; no ambient authority (Phase 2/D7).
- ☑ **No Rust panic** crosses the FFI boundary → op/timer/reject callbacks are `catch_unwind`-wrapped (Phase 9, resolves D15; assumes `panic = "unwind"`).
- ◐ **Intrinsic integrity** against prototype pollution / global tampering → prelude objects are frozen where built; a systematic audit/freeze pass is a follow-up.
- ☑ **Reproducibility** under deterministic providers (Phase 3 test providers).

---

## 5. Conformance & testing

- Unit tests per module; integration tests via `runtime-cli`.
- **Conformance:** ☑ a curated in-repo suite of spec-behaviour assertions over the implemented surface (`crates/runtime/conformance/*.js`), run by the `conformance_suite_passes` gate with a recorded, non-regressing pass-rate (`conformance/RESULTS.md` — currently 57/57). The full WPT harness (`testharness.js`) is a later addition; the curated suite is meant to trend up as coverage grows.
- **Fuzzing:** `cargo-fuzz` on URL parsing, streams, encoding, and the JS↔Rust marshaler.
- **Soundness:** Miri on the safe core where applicable; ASAN/valgrind on the FFI surface in CI; isolate/handle release verified.
- **CI gates (all required):** `cargo fmt --check`, `cargo clippy -D warnings`, tests, `cargo-deny`, `cargo-audit`, MSRV build, conformance run.

---

## 6. Phased roadmap

Each phase must compile, pass CI, and be independently reviewable. At each phase start, restate the plan and seek sign-off before locking any cross-cutting decision.

1. ☑ **Foundation** — workspace, `common`, error model, tracing, CI; `engine` V8 init running `"1+1"`; snapshot scaffolding.
2. ☑ **Op system + driven loop** — sync/async ops, promise resolution, microtask checkpoint, tick/poll API, timer plumbing. (`runtime` crate + engine trait introduced here; see DECISIONS D15.)
3. ☑ **Provider traits + default tokio providers** — Clock, Entropy, Timers, TaskSpawner; deterministic test providers. (`providers` + `default-providers` crates + a tokio `Driver`; `runtime` API unchanged — DECISIONS D16.)
4. ☑ **Core web primitives** — console, encoding, URL family, `structuredClone`, performance, events, Abort. (JS prelude over the op system + `Console` provider; DECISIONS D17/D18.)
5. ◐ **Streams** — readable/writable/transform + backpressure + queuing strategies + tee/pipe + encoding streams, hand-written (DECISIONS D19). Byte/BYOB streams deferred to a follow-up.
6. ◐ **Fetch family** — Headers/Request/Response/Body/fetch over `NetTransport` (reqwest+rustls), Blob/File/FormData (DECISIONS D20). Streamed response bodies; request-body streaming deferred.
7. ☑ **WebCrypto** — getRandomValues, randomUUID, subtle digest/HMAC/AES-GCM/AES-CBC/AES-CTR + HKDF/PBKDF2 derivation + ECDSA/ECDH (P-256/384/521) + RSA (PKCS1-v1_5/PSS/OAEP) (RustCrypto — DECISIONS D9). Carries the `rsa` Marvin advisory (SECURITY.md).
8. ☑ **Snapshot + perf** — the prelude + op shells bake into a V8 startup snapshot (D8); `Runtime::with_snapshot` restores it (~2.3× faster startup in the `bench` example). Zero-copy `ArrayBuffer` transfer was audited and deliberately deferred (D3a Phase 8). Benchmark harness (`default-providers` `bench` example) covers context creation + op-dispatch throughput.
9. ◐ **Hardening + conformance** — ☑ safety spine (heap/execution/stack limits + watchdog `InterruptHandle`, bounded pending-ops, panic-across-FFI containment; `esrun --timeout`); ☑ curated conformance suite + recorded pass-rate. Remaining: fuzzing (`cargo-fuzz`) + sanitizer CI (Miri/ASAN) — needs nightly; intrinsic-integrity audit; byte/BYOB streams; security review + docs finalization.

---

## 7. Non-goals & deferrals

**Non-goals (this repo):**
- No actor/process model, scheduler, preemption, mailboxes, supervisors (Layer B).
- No Node.js compatibility, npm, CommonJS, or `node:` modules.
- No self-owned event loop or thread management in `runtime`.
- No second engine yet (boundary kept clean for later JSC).
- No HTTP *server* — only the `fetch` client. Serving belongs to the embedder/Layer B.
- No `deno_core` or any pre-built runtime framework.

**Deferrals:**
- **Panic-across-FFI containment** (`catch_unwind` around op/timer/reject callbacks, per D12) — ☑ **implemented in Phase 9**: a host op panic is contained as a JS exception, not an abort (assumes `panic = "unwind"`). (DECISIONS D15.)
- **`DOMException` engine reconciliation** — the JS class exists (Phase 4 prelude), but errors thrown from the engine still surface as `Error` with a name-prefixed message. (DECISIONS D3a.)
- **Byte/BYOB streams** (`ReadableByteStreamController`, BYOB readers) → a streams follow-up (DECISIONS D19). Default streams + encoding streams ship in Phase 5.
- **Streaming `fetch` request bodies** → a follow-up; Phase 6 buffers the request body and streams the response (DECISIONS D20).
- **`crypto.subtle` minor gaps.** The algorithm set is complete (digest/HMAC/AES-GCM/CBC/CTR, HKDF/PBKDF2, ECDSA/ECDH, RSA PKCS1-v1_5/PSS/OAEP — DECISIONS D9). Remaining edges: AES-CTR supports only 32/64/128-bit counter widths (others → `NotSupportedError`); RSA-OAEP **labels must be UTF-8** (the `rsa` 0.9 API limitation; non-UTF-8 → `NotSupportedError`); EC keys import/export as raw/spki/pkcs8/jwk and RSA as spki/pkcs8/jwk; `deriveKey` targets AES-* and HMAC keys. All asymmetric signing/keygen randomness routes through the Entropy provider, never ambient `OsRng`. RSA carries an **accepted timing-sidechannel advisory** (RUSTSEC-2023-0071) tracked on the SECURITY.md revisit list.
- **`URLPattern`** → later (not covered by the `url` crate). Minor WHATWG URL conformance gaps tracked vs WPT (D18).
- **`reportError` ErrorEvent dispatch** and **sub-millisecond `performance.now`** are minimal in Phase 4; full behavior lands with the event loop / clock refinements.

---

## 8. Definition of done

- `runtime-cli` runs real ES modules using the full implemented WinterTC surface on the default tokio providers, end-to-end.
- `runtime` has **zero** direct `v8` dependency; all engine access via `engine`.
- All I/O is provider-routed; deterministic providers make runs reproducible.
- Limits + watchdog demonstrably stop a runaway / heap-bomb script without harming the host.
- CI green on every gate; conformance pass-rate recorded and trending up.
- `ARCHITECTURE.md`, `SPEC.md`, `DECISIONS.md`, `CHANGELOG.md` complete and current.
- A second engine could slot behind `engine` without changing `runtime`, verified by review, with leak points documented.
