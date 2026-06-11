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
- ☐ `crypto.getRandomValues` (Entropy provider), `crypto.randomUUID`.
- ☐ `crypto.subtle`: digest, HMAC, AES-GCM/CBC, ECDSA/ECDH, RSA per spec — vetted constant-time library only.

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

- Per-isolate **heap limit** → graceful termination on near-limit; host never OOMs.
- **Execution-time / CPU watchdog** → runaway script terminated; surfaces as a typed error, never a hang.
- **Stack-depth** guard.
- **Bounded pending-op** concurrency.
- **Deny-by-default** capabilities; no ambient authority.
- **No Rust panic** crosses the FFI boundary.
- **Intrinsic integrity** against prototype pollution / global tampering.
- **Reproducibility** under deterministic providers.

---

## 5. Conformance & testing

- Unit tests per module; integration tests via `runtime-cli`.
- **Conformance:** Minimum Common Web API / WPT subset per implemented API; pass-rate tracked in-repo and trending up.
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
7. **WebCrypto** — getRandomValues, randomUUID, subtle.
8. **Snapshot + perf** — bake prelude into snapshot; zero-copy audit; benchmark context creation + op throughput.
9. **Hardening + conformance** — limits, watchdog, fuzzing, WPT run, security review, sanitizer CI, docs finalization.

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
- **Panic-across-FFI containment** (`catch_unwind` around op/timer/reject callbacks, per D12) is implemented in the **hardening phase (§6.9)**, not Phase 2. A *host-written* op handler that panics currently aborts the process; hostile JS cannot force this. (DECISIONS D15.)
- **`DOMException` engine reconciliation** — the JS class exists (Phase 4 prelude), but errors thrown from the engine still surface as `Error` with a name-prefixed message. (DECISIONS D3a.)
- **Byte/BYOB streams** (`ReadableByteStreamController`, BYOB readers) → a streams follow-up (DECISIONS D19). Default streams + encoding streams ship in Phase 5.
- **Streaming `fetch` request bodies** → a follow-up; Phase 6 buffers the request body and streams the response (DECISIONS D20).
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
