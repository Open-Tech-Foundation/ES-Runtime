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
- ☐ `globalThis` wiring, `queueMicrotask`, `structuredClone`, `reportError`.

### 2.2 Console
- ☐ `console` (log/info/warn/error/debug; group/table best-effort) → routed to the embedder's logging sink, not stdout directly.

### 2.3 Encoding
- ☐ `TextEncoder`, `TextDecoder`, `TextEncoderStream`, `TextDecoderStream`, `atob`, `btoa`.

### 2.4 URL
- ☐ `URL`, `URLSearchParams`, `URLPattern`.

### 2.5 Timers (provider-backed)
- ◐ `setTimeout`, `clearTimeout`, `setInterval`, `clearInterval`. Mechanism in place (Phase 2): engine builtins + runtime-owned schedule, embedder-supplied time. Provider-backing (`Clock`/`Timers`) lands in Phase 3.

### 2.6 Abort
- ☐ `AbortController`, `AbortSignal` (incl. `AbortSignal.timeout`, `AbortSignal.any`).

### 2.7 Events
- ☐ `Event`, `EventTarget`, `CustomEvent`.

### 2.8 Streams (largest correctness item)
- ☐ `ReadableStream` (default + **byte/BYOB** streams), `WritableStream`, `TransformStream`, with correct **backpressure** and queuing strategies (`CountQueuingStrategy`, `ByteLengthQueuingStrategy`).

### 2.9 Fetch family
- ☐ `Headers`, `Request`, `Response`, `Body` mixin, `fetch` — networking exclusively via the `NetTransport` provider; streaming bodies via §2.8.
- ☐ `Blob`, `File`, `FormData`.

### 2.10 WebCrypto
- ☐ `crypto.getRandomValues` (Entropy provider), `crypto.randomUUID`.
- ☐ `crypto.subtle`: digest, HMAC, AES-GCM/CBC, ECDSA/ECDH, RSA per spec — vetted constant-time library only.

### 2.11 Performance
- ☐ `performance.now()`, `performance.timeOrigin` (Clock provider).

Anything intentionally deferred from the snapshot is listed in §7 with rationale.

---

## 3. I/O provider contracts

Traits the embedder must satisfy (defaults shipped in `default-providers`):

- `Clock` — wall + monotonic time.
- `Entropy` — CSPRNG bytes.
- `Timers` — schedule/cancel.
- `NetTransport` — outbound HTTP for `fetch`.
- `FileSystem` — capability-scoped, async, optional/deniable.
- `TaskSpawner` — offload blocking work.

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
3. **Provider traits + default tokio providers** — Clock, Entropy, Timers, TaskSpawner; deterministic test providers.
4. **Core web primitives** — console, encoding, URL family, `structuredClone`, performance, events, Abort.
5. **Streams** — full readable/writable/transform incl. byte streams + backpressure.
6. **Fetch family** — Headers/Request/Response/Body/fetch over NetTransport; Blob/File/FormData.
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
- **`DOMException` as a real JS class** awaits the runtime prelude (Phase 4). Until then, `DOMException`-classed errors (e.g. capability denial → `NotAllowedError`) surface as `Error` with a name-prefixed message. (DECISIONS D3a.)
- **`queueMicrotask` / `reportError`** globals (§2.1) not yet installed; the microtask *checkpoint* mechanism exists (Phase 2), the `globalThis` bindings come with §2.1.

---

## 8. Definition of done

- `runtime-cli` runs real ES modules using the full implemented WinterTC surface on the default tokio providers, end-to-end.
- `runtime` has **zero** direct `v8` dependency; all engine access via `engine`.
- All I/O is provider-routed; deterministic providers make runs reproducible.
- Limits + watchdog demonstrably stop a runaway / heap-bomb script without harming the host.
- CI green on every gate; conformance pass-rate recorded and trending up.
- `ARCHITECTURE.md`, `SPEC.md`, `DECISIONS.md`, `CHANGELOG.md` complete and current.
- A second engine could slot behind `engine` without changing `runtime`, verified by review, with leak points documented.
