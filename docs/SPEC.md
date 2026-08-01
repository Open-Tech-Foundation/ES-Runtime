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
- ◐ `globalThis` wiring (+ `self`) ☑, `queueMicrotask` ☑, `structuredClone` ☑ (standard cloneable types + cycles), `navigator.userAgent` ☑ (`"ES-Runtime/<version>"`, substituted from the crate version; the rest of the browser `Navigator` is deliberately absent — §7), `reportError` ☑ (dispatches a cancelable `error` `ErrorEvent` on the global scope, falling back to `console.error` when nothing claims it). *(Phase 4.)*
- ☑ **Failures that reach the global scope.** An exception out of a timer callback fires a cancelable `error` (`ErrorEvent`); a promise rejection with no handler fires a cancelable `unhandledrejection` (`PromiseRejectionEvent`), and attaching a handler after the report has gone out fires `rejectionhandled`. `preventDefault()` is how guest code takes responsibility: a claimed failure is not reported to the embedder. `onerror`/`onunhandledrejection`/`onrejectionhandled` are single-handler slots over the same events. What no listener claims reaches the embedder on `TickStatus` (`unhandled_rejections`, `uncaught_errors`); `esrun` prints it and exits non-zero.

### 2.2 Console
- ☑ `console` — the full Console Standard method set, routed to the injected `Console` sink rather than stdout (DECISIONS D17): the log family, `dir`/`dirxml`, `trace` (with a stack), `assert`, `group`/`groupCollapsed`/`groupEnd` (indenting subsequent output, per line), `count`/`countReset`, `time`/`timeLog`/`timeEnd`, `table` (rendered), `clear`, and the `%s`/`%d`/`%i`/`%f`/`%o`/`%O`/`%j`/`%c` format specifiers. *(Phase 4.)*

### 2.3 Encoding
- ☑ `TextEncoder` (UTF-8, as the spec fixes it), `TextDecoder` — **every encoding and label the WHATWG Encoding Standard defines** (UTF-8/16LE/16BE, the single-byte legacy sets, and the multi-byte CJK ones), via `encoding_rs`; `fatal`, `ignoreBOM` and streaming decode across chunk boundaries all honoured. `atob`, `btoa` *(Phase 4)*; `TextEncoderStream`/`TextDecoderStream` *(Phase 5, on `TransformStream`)*.

### 2.4 URL
- ☑ `URL`, `URLSearchParams` (via the `url` crate — D18), `URL.createObjectURL`/`revokeObjectURL` (an in-process `blob:` store; no `Net` capability, since nothing leaves the isolate), and `URLPattern` (via the `urlpattern` crate, with V8 compiling the emitted component regexes — **369/369 on the official WPT suite**). Component parsing, relative-reference resolution, default-port dropping, and `hostname`/`host` setter port handling are covered by the conformance suite. *(Phase 4.)*

### 2.5 Timers (provider-backed)
- ☑ `setTimeout`, `clearTimeout`, `setInterval`, `clearInterval` — engine builtins over a runtime-owned schedule with embedder-supplied time, provider-backed by `Clock`/`Timers` (Phase 3). Trailing arguments are forwarded to the callback, and clearing a timer releases the loop immediately rather than at its original deadline.

### 2.6 Abort
- ☑ `AbortController`, `AbortSignal` (incl. `AbortSignal.timeout`, `AbortSignal.any`). *(Phase 4.)*

### 2.7 Events
- ☑ `Event`, `EventTarget`, `CustomEvent` (flat dispatch model). *(Phase 4.)*

### 2.8 Streams (largest correctness item)
- ☑ `ReadableStream` (default + **byte/`type:"bytes"`**), `WritableStream`, `TransformStream`, **backpressure**, `CountQueuingStrategy`/`ByteLengthQueuingStrategy`, `tee`/`pipeTo`/`pipeThrough`, and **byte/BYOB** streams (`ReadableByteStreamController`, `ReadableStreamBYOBReader`, `ReadableStreamBYOBRequest`, `autoAllocateChunkSize`) *(Phase 5 + Phase 9, hand-written — DECISIONS D19)*. Byte streams copy rather than transfer/detach ArrayBuffers (single-threaded; zero-copy is the D3a follow-up). Source/sink/transformer methods run with **promise-calling semantics** (a synchronous throw becomes a rejection), and `transformer.cancel` runs on writable-abort / readable-cancel.
- ☑ **Compression Streams**: `CompressionStream`/`DecompressionStream` for all four spec format tokens — `brotli`, `gzip`, `deflate` (zlib), `deflate-raw` — over stateful native codec contexts (flate2 + the pure-Rust `brotli` crate) behind pure sync ops (no capability); decompression errors on corrupt input and trailing junk at write time and on truncation at flush; the transformer-cancel hook frees the native context on abort/cancel.

### 2.9 Fetch family
- ☑ `Headers`, `Request`, `Response`, `Body` mixin, `fetch` — networking exclusively via the `NetTransport` provider. **Both** directions stream: response bodies via §2.8, and a `ReadableStream` **request** body uploads with chunked transfer-encoding (bounded-channel backpressure) rather than buffering. A non-stream body still travels buffered. *(Phase 6, DECISIONS D20.)*
- ☑ **Connect bounded.** The default transport caps DNS+TCP+TLS at 30s (`ERR_TIMED_OUT`) and keeps a 60s TCP keepalive on pooled connections. The request as a whole is deliberately **uncapped**: Fetch defines no timeout and a response body may be long-lived by design (SSE, a log tail), so a total deadline would break correct programs — `AbortSignal.timeout(ms)` is the caller's tool for that.
- ☑ **Content-codings** (Fetch's "decode" step). The default transport sends `Accept-Encoding: gzip, br, deflate` — the same set `CompressionStream` implements (§2.8) — and decodes a response in any of them off its `Content-Encoding`, dropping `Content-Encoding`/`Content-Length` so they cannot describe bytes the guest never sees. A coding the client does not implement passes through untouched. `zstd` is deliberately absent: nothing else in the runtime speaks it. Outbound requests carry `User-Agent: ES-Runtime/<version>`, matching `navigator.userAgent`, unless the request sets its own. Both are properties of `ReqwestTransport`, not of the `NetTransport` contract.
- ☑ **Redirects.** All three `RequestRedirect` modes are honoured, and the mode reaches the transport (`HttpRequest.redirect`) so a refused redirect is never walked: `"follow"` up to the spec's cap of 20 (past it, `ERR_TOO_MANY_REDIRECTS`), `"manual"` resolving with the unfollowed `3xx`, `"error"` rejecting with a `TypeError`. `Response.redirected` and the final `Response.url` come from the transport; an unknown mode is a `TypeError` from the `Request` constructor. One deviation, recorded in §7: `"manual"` returns the real response rather than an opaque-redirect filtered one.
- ☑ `Blob`, `File`, `FormData`. *(Phase 6.)*

### 2.10 WebCrypto
- ☑ `crypto.getRandomValues` (Entropy provider), `crypto.randomUUID`. *(Phase 7.)*
- ☑ `crypto.subtle`: digest (SHA-1/256/384/512), HMAC, AES-GCM, AES-CBC, AES-CTR, **AES-KW**, `wrapKey`/`unwrapKey`, `deriveBits`/`deriveKey` via HKDF + PBKDF2, ECDSA + ECDH over P-256/P-384/P-521, **Ed25519** signatures and **X25519** agreement (the Secure Curves), and RSA (RSASSA-PKCS1-v1_5, RSA-PSS, RSA-OAEP) — raw/spki/pkcs8/jwk key formats (symmetric keys as `kty: "oct"`, the Secure Curves as `kty: "OKP"`) *(Phase 7/7b, RustCrypto — DECISIONS D9)*. RSA carries an accepted timing-sidechannel advisory (SECURITY.md); RSA-OAEP labels are UTF-8 only (§7).

### 2.11 Performance
- ☑ `performance.now()`, `performance.timeOrigin` (Clock provider) — sub-millisecond (fractional-ms) resolution. *(Phase 4.)*

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
- ☑ `FileSystem` — capability-scoped (`FileRead`/`FileWrite`), async, optional/deniable; backs `runtime:fs`. *(Phase 11: trait + `SystemFs`, root-jailed per D25.)*
- ☑ `SyncFileSystem` — the blocking seam WASI's syscalls need, same gates and jail (DECISIONS D36). *(Phase 12.)*
- ☑ `Process` — env/args/cwd/platform/exit, gated on `Env` (DECISIONS D26). *(Phase 11.)*
- ☑ `Signals` — OS signal delivery (`SIGINT`/`SIGTERM`/`SIGHUP`/`SIGUSR1`/`SIGUSR2`; `SIGINT`/`SIGBREAK` on Windows), pull-based like `next_requests` since the runtime owns no loop. Gated on the new `Signals` capability, **separate from `Env`**: a watch suppresses the signal's default action, so it is the privilege to decline to die on request rather than a read of process state. Defaults `SystemSignals` (tokio) and `ManualSignals` (deterministic).
- ☑ `NetProvider` — sockets + listener for `runtime:net`, gated on `Net`/`NetListen` (DECISIONS D28). *(Phase 12.)*
- ☑ `HttpServerProvider` — the `runtime:http` `serve()` seam, streaming both directions (DECISIONS D31), plus `request_disconnected` backing the handler's `request.signal`. That method has a default returning `false`, so a transport with no way to observe its peer keeps compiling and simply has a signal that never fires. *(Phase 12.)*
- ☑ `WebSocketProvider` — client and server framing for `WebSocket` / `runtime:websocket` (DECISIONS D29). *(Phase 12.)*

All calls: async-friendly, cancellable, capability-checked, typed errors. No provider, no capability ⇒ clean JS exception.

---

## 4. Resource limits & security guarantees

- ☑ Per-isolate **heap limit** → near-limit guard terminates execution before the host OOMs (Phase 9: `add_near_heap_limit_callback` → `Error::Terminated`).
- ☑ **Execution-time watchdog** → a runaway script is terminated via a thread-safe `InterruptHandle`; surfaces as `Error::Terminated`, never a hang (Phase 9). CPU-cycle accounting (vs wall-clock) is not separately implemented.
- ☑ **Stack-depth** guard → V8-native; unbounded recursion is a catchable `RangeError`, not UB or a hang (Phase 9 test).
- ☑ **Bounded pending-op** concurrency → `max_pending_ops`; the over-limit async dispatch throws `RangeError` (Phase 9).
- ☑ **Deny-by-default** capabilities; no ambient authority (Phase 2/D7). The *embeddable library* starts from `CapabilitySet::none()`; `esrun` starts from all-granted and narrows on request (D38).
- ☑ **`esrun` denial flags** → `--deny-all` (no host access at all) **or** one or more `--deny-<name>` from `read, write, imports, net, listen, env, run, signals` — never both (D38). A denial is a `NotAllowedError` / `ERR_CAPABILITY_DENIED` thrown before the effect, never a partial one. Denials are coarse; scoped values (`--deny-net=<hosts>`) are deferred.
- ☑ **Importing a `runtime:` module never needs a capability** → the gate is the op, so a built-in imports even under `--deny-all` and only its operations throw (D26/D38, tested against `CapabilitySet::none()` for every built-in).
- ☑ **No Rust panic** crosses the FFI boundary → op/timer/reject callbacks are `catch_unwind`-wrapped (Phase 9, resolves D15; assumes `panic = "unwind"`).
- ◐ **Intrinsic integrity** against prototype pollution / global tampering → **the load-bearing guarantee holds (☑): the op table and the capability set live in Rust `OpState`, not in JS, so no guest tampering (prototype pollution, global reassignment, forging `__ops`) can escalate privilege or dispatch an ungated op** (tested). JS-surface defense-in-depth: the `__ops` binding is locked and namespace objects (`console`/`crypto`/`performance`) frozen (`harden.js`). **Deferred:** SES-style primordial freezing (hardening the *prelude's own* correctness against `Object`/`Array.prototype` pollution) is left to the embedder / Layer B rather than baked into a general-purpose Layer A (SECURITY.md).
- ☑ **Reproducibility** under deterministic providers (Phase 3 test providers).

---

## 5. Conformance & testing

- Unit tests per module; integration tests via `runtime-cli`. The behavioral test
  job runs on **Linux, Windows, and macOS** (CI matrix) so platform-divergent
  surfaces — filesystem/path semantics, the symlink-canonicalized root jail,
  process exit codes, networking, CRLF/encoding — are covered, not just Linux.
- **Conformance:** ☑ a curated in-repo suite of spec-behaviour assertions over the implemented surface (`crates/runtime/conformance/*.js`), run by the `conformance_suite_passes` gate with a recorded, non-regressing pass-rate (`conformance/RESULTS.md` — currently 278/278). The same files and harness also run under the real CLI (`esrun crates/runtime/conformance/run.js`), which exits non-zero on failure; the `cargo test` runner is the gate, the `esrun` one is the driven second opinion. The full WPT harness (`testharness.js`) is a later addition; the curated suite is meant to trend up as coverage grows.
- **Soak / leak:** ◐ opt-in soak tests (`#[ignore]`, run with `cargo test -- --ignored soak`) hammer a subsystem over many iterations and assert it neither leaks nor deadlocks. The first, `soak_streaming_fetch_does_not_leak`, runs 20k streaming-`fetch` uploads and asserts (a) the three request/response **body registries drain to zero every iteration** — the precise native-leak guard, via the test-only `__fetch_inflight` op — and (b) steady-state RSS stays bounded. Broad cross-subsystem soak coverage is a Phase-14 item.
- **Fuzzing:** ☑ `cargo-fuzz` targets under `fuzz/` over the parsers that read untrusted bytes — URL parsing + component read-back, `TextDecoder` across every label, URLPattern constructor strings, decompression, XML, and the hand-written RFC 8410 key DER + `atob`. A CI job runs each for 60s seeded from the committed `fuzz/seeds/`, which includes every input that has found a bug. *Not* fuzzed: the JS↔Rust marshaler and streams, which need a live isolate — standing V8 up per iteration would cut the rate by orders of magnitude and fuzz V8 rather than this code; they are covered by the conformance suite and the Rust tests instead.
- **Soundness:** ☑ Miri in CI over `common` + `providers` — the crates that do not link V8. ◐ The FFI surface is not yet under ASAN: the `v8` crate links a **prebuilt** static library, and running a sanitizer against uninstrumented native code reports the library's own allocations rather than our misuse of it. Doing it properly means building V8 from source with `-fsanitize=address`, which is a multi-hour build and a separate pipeline; recorded here rather than pretended. Isolate/handle release is verified by the leak soak.
- **CI gates (all required):** `cargo fmt --check`, `cargo clippy -D warnings`, tests (Linux/Windows/macOS), `cargo-deny`, `cargo-audit`, MSRV build, **`cargo-fuzz`** (each target for 60s from the committed seeds), **Miri**, conformance run (both the `cargo test` gate and the driven `esrun` runner), and the **JS job** — `bun test` over the TypeScript sources under `crates/runtime/js` plus a rebuild of the committed `runtime:serialization` bundle asserting it matches those sources.

---

## 6. Phased roadmap

Each phase must compile, pass CI, and be independently reviewable. At each phase start, restate the plan and seek sign-off before locking any cross-cutting decision.

1. ☑ **Foundation** — workspace, `common`, error model, tracing, CI; `engine` V8 init running `"1+1"`; snapshot scaffolding.
2. ☑ **Op system + driven loop** — sync/async ops, promise resolution, microtask checkpoint, tick/poll API, timer plumbing. (`runtime` crate + engine trait introduced here; see DECISIONS D15.)
3. ☑ **Provider traits + default tokio providers** — Clock, Entropy, Timers, TaskSpawner; deterministic test providers. (`providers` + `default-providers` crates + a tokio `Driver`; `runtime` API unchanged — DECISIONS D16.)
4. ☑ **Core web primitives** — console, encoding, URL family, `structuredClone`, performance, events, Abort. (JS prelude over the op system + `Console` provider; DECISIONS D17/D18.)
5. ☑ **Streams** — readable/writable/transform + backpressure + queuing strategies + tee/pipe + encoding streams, hand-written (DECISIONS D19). Byte/BYOB streams added in Phase 9.
6. ☑ **Fetch family** — Headers/Request/Response/Body/fetch over `NetTransport` (reqwest+rustls), Blob/File/FormData (DECISIONS D20). Streamed response **and** request bodies (chunked upload with backpressure).
7. ☑ **WebCrypto** — getRandomValues, randomUUID, subtle digest/HMAC/AES-GCM/AES-CBC/AES-CTR + HKDF/PBKDF2 derivation + ECDSA/ECDH (P-256/384/521) + RSA (PKCS1-v1_5/PSS/OAEP) (RustCrypto — DECISIONS D9). Carries the `rsa` Marvin advisory (SECURITY.md).
8. ☑ **Snapshot + perf** — the prelude + op shells bake into a V8 startup snapshot (D8); `Runtime::with_snapshot` restores it (~2.3× faster startup in the `bench` example). Zero-copy `ArrayBuffer` transfer was audited and deliberately deferred (D3a Phase 8). Benchmark harness (`default-providers` `bench` example) covers context creation + op-dispatch throughput.
9. ◐ **Hardening + conformance** — ☑ safety spine (heap/execution/stack limits + watchdog `InterruptHandle`, bounded pending-ops, panic-across-FFI containment; `esrun --timeout`); ☑ curated conformance suite + recorded pass-rate. ☑ byte/BYOB streams; ☑ intrinsic-integrity audit (Rust-side boundary verified + JS-surface defense-in-depth; SES-style primordial hardening deferred to the embedder); ☑ internal security review (`docs/SECURITY-REVIEW.md`) + docs finalization. ☑ fuzzing (`cargo-fuzz`, six targets) + Miri in CI (both on nightly). Remaining: ASAN over the FFI surface (needs a source build of V8 with `-fsanitize=address` — §5) and an **external** security review (pre-`1.0`).

### v1 standalone roadmap (phases 10–14, DECISIONS D24)

Productionizing the standalone runtime *and* stabilizing the embeddable API. ESM module support (static + dynamic, `node_modules` ESM) landed ahead of these (D21/D22/D23).

10. ◐ **FS sandbox + symlink-correct resolution** — module/file resolution **realpaths** resolved modules (Node-default, preserve-symlinks off) so pnpm's symlinked store resolves transitive deps; resolution is **root-jailed** to the detected project root by default (DECISIONS D25). The behavioral test job now runs on **Linux + Windows + macOS** (CI matrix), so the platform-divergent path/jail behavior is exercised on each.
11. ☑ **`runtime:` standard modules I** — ☑ the `runtime:` built-in scheme (served by the runtime, loader-independent; ops are the capability boundary) and ☑ **`runtime:process`** (`env` mutable-in-process / `args` / `cwd()` / `platform` OS-native / `exit(code=0)`), gated on `Capability::Env`, backed by the new `Process` provider (DECISIONS D26). ☑ **`runtime:path`** (pure; uses `cwd`/`platform`) and ☑ **`runtime:fs`** (async file ops, jailed, incl. `Glob`, `copy`, `realPath`, `readLink`, `truncate`, `chmod`, and jailed `makeTempDir`/`makeTempFile`) over the new `FileSystem` provider. `copy` names **both** `FileRead` and `FileWrite` — ops may now require more than one capability, since gating a read-and-write call on the write alone would let a guest duplicate a file it cannot see. Phase 11 is complete.
12. ☑ **`runtime:` standard modules II** — ☑ **`runtime:serialization`** (XML validation, XML parser, and XML builder backed natively by `quick-xml`). ☑ **`runtime:net`** (sockets + listener provider, client/server TLS per the WinterTC Sockets API; DECISIONS D28). ☑ **`runtime:http`** (the HTTP **server** capstone — `serve((req) => res)` over the `HttpServerProvider` seam, bodies **streaming both directions** with backpressure; DECISIONS D31), including **TLS termination** (`secureTransport: "on"` with an inline PEM `cert`/`key` and `alpn`, sharing `runtime:net`'s server-side rustls setup) — so `request.url` reports `https:`, a bad cert fails the bind rather than each handshake, and a failed handshake ends only its own connection. HTTP/1.1 only; no HTTP/2. ☑ Streaming `fetch` request bodies (chunked upload with backpressure; DECISIONS D20).
13. ☑ **Diagnostics & DX** — error model standardization. ☑ JS **stack traces + source position** (`engine` extracts `Error.stack` with a `v8::Message` `file:line:col` fallback, preserving the error class), ☑ **one coherent CLI error block** with ☑ **optional color** (bold-red `error`, dimmed `at …` frames; terminal-detected + `NO_COLOR`-aware). ☑ **Stable guest-facing error codes**: a documented `ErrorCode` set in `common` (`ERR_NOT_FOUND`, `ERR_CAPABILITY_DENIED`, `ERR_JAIL_ESCAPE`, `ERR_TLS`, …, API.md §Error codes) surfaces as an own `code` string property on the thrown JS exception via `IntoException::exception_code` — messages stay prose, the code is the contract; providers classify io/TLS/DNS failures (`ProviderError::Coded`/`from_io`), and an unclassified error simply carries no code. (SPEC §7 deferral promoted.)
14. ◐ **Production hardening & release** — ☑ **graceful shutdown**: `esrun` handles `^C`/`SIGTERM` by stopping accepting, draining in-flight HTTP requests, and exiting `128+signal`, bounded by `--shutdown-grace` (default 10s). A guest signal handler takes it over entirely; with no server running the exit stays immediate, so a plain script is unaffected. Draining waits for the *connections* to close, not just for the handler to return — a response is handed to the transport before it reaches the socket. ☑ cross-platform test CI (Linux + Windows + macOS matrix); ◐ soak/leak tests (the streaming-`fetch` leak soak landed; broad cross-subsystem coverage remains). ☑ fuzzing + Miri in CI. Remaining: ASAN over the FFI surface (§5), external security review, API freeze + semver commitment, embedder guide + supported-platforms statement. *(A standard **WPT subset** is **deferred to post-1.0**: this is a server-side WinterTC runtime, so full Web Platform Tests — built around DOM/document/worker semantics and legacy encodings — are disproportionate; the curated in-repo `conformance/*.js` suite is the pre-1.0 conformance signal and keeps trending up.)*

---

## 7. Non-goals & deferrals

**Non-goals (this repo):**
- No actor/process model, scheduler, preemption, mailboxes, supervisors (Layer B).
- No Node.js compatibility, CommonJS, or `node:` modules. **(Amended, D22:** bare specifiers resolve against an existing `node_modules` tree for **ES module** packages only — CommonJS packages and `node:` builtins are rejected, and nothing is installed. No CJS interop, no `node:` builtins, no npm client.**)**
- No self-owned event loop or thread management in `runtime`.
- No second engine yet (boundary kept clean for later JSC).
- No HTTP *server* — only the `fetch` client. Serving belongs to the embedder/Layer B. **(Superseded:** `runtime:http` `serve()` shipped as a capability-gated standard module over the `HttpServerProvider` seam — the embedder still owns the transport; DECISIONS D31.**)**
- No `deno_core` or any pre-built runtime framework.

**Deferrals:**
- **Panic-across-FFI containment** (`catch_unwind` around op/timer/reject callbacks, per D12) — ☑ **implemented in Phase 9**: a host op panic is contained as a JS exception, not an abort (assumes `panic = "unwind"`). (DECISIONS D15.)
- **`DOMException` engine reconciliation** — ☑ **implemented**: the engine dynamically resolves `globalThis.DOMException` when marshaling a native `DOMException`, surfacing it as a proper instance of the JS class (resolves DECISIONS D3a).
- **Byte/BYOB streams** (`ReadableByteStreamController`, BYOB readers) — ☑ **implemented in Phase 9** (copy-based, no ArrayBuffer transfer/detach; DECISIONS D19). Default streams + encoding streams shipped in Phase 5.
- **Fetch redirect modes** → ☑ **implemented** (§2.9). One deliberate deviation:
  under `redirect: "manual"` the specification returns an **opaque-redirect
  filtered** response (status `0`, no headers, null body). That filtering exists
  so a browser can hand a redirect to its navigation machinery without leaking
  cross-origin data; this runtime has no navigation and no origin to protect,
  and the filtered response would make the mode useless — the reason to ask for
  `"manual"` server-side is to read `Location`. The real `3xx` is returned
  instead, matching Node, Deno and Bun. `Response.type` stays `"default"`;
  `"opaqueredirect"` is never produced.
- **Streaming `fetch` request bodies** → ☑ **implemented**: a `ReadableStream`
  request body uploads with chunked transfer-encoding, pumped to the host one
  chunk at a time over a bounded channel (upload backpressure); a stream error
  aborts the in-flight request. The provider's `HttpRequest.body` is now a
  `RequestBody` enum (`Empty`/`Bytes`/`Stream`). Non-stream bodies stay buffered.
  (DECISIONS D20.)
- **Streaming `runtime:http` server bodies** → ☑ **implemented**: the handler's
  `Request` body is a `ReadableStream` pulled from the host chunk-by-chunk, and a
  `ReadableStream` response body is sent with chunked transfer-encoding over a
  bounded push channel (download backpressure) — `new Response(request.body)`
  proxies unbuffered. The provider seam's `HttpServerRequest`/`HttpServerResponse`
  bodies are now an `HttpServerBody` enum (`Empty`/`Bytes`/`Stream`). Buffered
  bodies stay the fast path. (DECISIONS D31.)
- **`crypto.subtle` minor gaps.** The algorithm set is complete (digest/HMAC/AES-GCM/CBC/CTR, HKDF/PBKDF2, ECDSA/ECDH, RSA PKCS1-v1_5/PSS/OAEP — DECISIONS D9). Remaining edges: AES-CTR supports only 32/64/128-bit counter widths (others → `NotSupportedError`); RSA-OAEP **labels must be UTF-8** (the `rsa` 0.9 API limitation; non-UTF-8 → `NotSupportedError`); EC keys import/export as raw/spki/pkcs8/jwk and RSA as spki/pkcs8/jwk; `deriveKey` targets AES-* and HMAC keys. All asymmetric signing/keygen randomness routes through the Entropy provider, never ambient `OsRng`. RSA carries an **accepted timing-sidechannel advisory** (RUSTSEC-2023-0071) tracked on the SECURITY.md revisit list.
- **`runtime:net` TLS** → implemented per the WinterTC Sockets API (DECISIONS D28). **In:** `secureTransport: "on"` client TLS (certificate verification, **SNI**, **ALPN** surfaced as `SocketInfo.alpn`), the `Socket.upgraded` flag, `startTls()` / `secureTransport: "starttls"` in-place upgrade, **server-side TLS termination on `listen`** (`{ secureTransport: "on", cert, key, alpn }`, cert/key inline so no capability beyond `NetListen`), `allowHalfOpen`, and the combined `"host:port"` `SocketInfo` shape. The TLS surface is complete. The remaining spec-letter details are also covered: the advisory `close(reason?)` argument, and `SocketError` (failures surface as a `TypeError` whose message is prefixed `"SocketError: "`). The `runtime:net` surface fully matches the WinterTC Sockets proposal.
  - **Perf follow-up (cache-optimization phase):** TLS `connect` currently builds a fresh `ClientConfig` + crypto provider **per connection** (`SystemNet::tls_connector`), cloning the webpki root store each time. Micro-bench the per-connect setup cost; if non-trivial, cache the `TlsConnector` keyed by the ALPN tuple. Internal optimization only — no API change.
- **WebSocket** → ☑ **implemented** (DECISIONS D29). The classic WHATWG `WebSocket` interface ships as a prelude global, bridging its push-based `message`/`close` events onto our pull-based op seam via an internal receive-pump that rides the existing tick contract (D4) — no owned loop. `ws_connect` gated on `Capability::Net`; default transport (`SystemWebSocket`) over `tokio-tungstenite` + the D28 `tokio-rustls` stack for `wss:`. A **server** is also shipped as `runtime:websocket` `serve()` (`Capability::NetListen`, `ws:` only), accepting connections that reuse the client send/recv/close seam, with a batched `broadcast(connections, data)` for chat-style fan-out (one host crossing, concurrent enqueue, coalesced writes — full delivery). ☑ **`WebSocketStream`** also ships as a prelude global (same `ws_connect`/`ws_send`/`ws_recv`/`ws_close` seam): `opened` resolves to `{ readable, writable, protocol, extensions }` with pull-based reads (real receive backpressure) and writes that await the host send; `closed` settles with `{ closeCode, reason }` (a post-close internal drain keeps receiving until the peer's close frame so it settles without a reader); `WebSocketError` (a `DOMException` subclass carrying `closeCode`/`reason`) is the failure type. **Deferred:** permessage-deflate (`extensions`), classic-`WebSocket` backpressure beyond a best-effort `bufferedAmount`, a `wss:` server, and pub/sub topics over the explicit-set broadcast.
- WHATWG URL — `hostname`/`host` setter port handling is resolved, IDN→punycode (ToASCII) works, and the implemented surface (parsing, relative resolution, default-port dropping, setters) is gated by the conformance suite; only long-tail WPT percent-encoding/normalization edges remain untracked (D18).
- **ES module loading** — ☑ **implemented**: static `import`/`export`, **dynamic `import()`** (resolving with the module namespace after the imported module fully evaluates; shares instances with static imports via the realm module map), `import.meta.url`, **`import.meta.resolve`**, native top-level await, **local `file:` modules**, **JSON modules via `with { type: "json" }`** (transpiled natively), and **`node_modules` resolution for ES module packages** via the capability-checked `ModuleLoader` provider (DECISIONS D21, D22, D23). `exports` resolution covers string targets, the `import`/`default` conditions, and **subpath patterns** (`"./*"`). **Deferred:** the remaining `node_modules` edges (full condition precedence beyond `import`/`default`, `imports`/#internal, self-reference), and **`import.meta.resolve` for bare specifiers** — resolving one means reading `package.json` files and probing the filesystem, which is host I/O, and `resolve` is synchronous; it throws a `TypeError` naming the specifier rather than answering with a URL it never resolved. Relative, absolute-path and absolute-URL specifiers resolve, with no I/O and no existence check, exactly as Node's does. **Rejected by design:** CommonJS packages, remote (`http:`) modules, and `node:` builtins (§125).
- **`reportError` ErrorEvent dispatch** → ☑ **implemented**, along with the rest of the global scope's error reporting: `error` for an exception out of a timer callback, `unhandledrejection`/`rejectionhandled` for promise rejections, each cancelable where the spec makes it so, with `preventDefault()` as the guest's way to take responsibility (§2.1). One deliberate deviation: `rejectionhandled` **does not retract** the embedder's report. The report goes out when the rejection is unclaimed at the end of a tick; a handler attached later tells the guest the report has been superseded, but `esrun` still exits non-zero — the same stance Node and Deno take on a rejection that was unhandled when it mattered. (`performance.now` sub-millisecond resolution is now implemented, §2.11.)
  - Phase 13 (diagnostics) spans `engine` (☑ stack/position + error-class preservation + the `code` property on marshaled exceptions), `runtime`/`providers`/`common` (☑ typed errors carrying stable `ErrorCode`s), and `runtime-cli` (☑ coherent error-block formatting + color). Phase 13 is complete.

---

## 8. Definition of done

- ☑ `runtime-cli` (`esrun`) runs JavaScript using the full implemented WinterTC surface on the default tokio providers, end-to-end. Inputs run as **ES modules** (`import`/`export`, dynamic `import()`, JSON imports, `import.meta.url`, native top-level `await`); imports resolve via `NodeModuleLoader` — local files (relative/absolute paths or `file:` URLs) plus bare specifiers through `node_modules` for **ES module** packages (D22) — gated on `Capability::FileSystem`. *Rejected by design:* CommonJS packages, remote (`http:`) modules, and `node:` builtins (§125). See DECISIONS D21/D22/D23; running every input as a module is a deliberate break from the prior classic-script behaviour (module scope: strict mode, `this === undefined`).
- ☑ `runtime` has **zero** direct `v8` dependency; all engine access via `engine` (verified by review — `runtime` names no V8 type).
- ☑ All I/O is provider-routed; deterministic providers make runs reproducible.
- ☑ Limits + watchdog demonstrably stop a runaway / heap-bomb script without harming the host (engine tests + `esrun --timeout`).
- ☑ CI green on every gate; conformance pass-rate recorded and trending up (`conformance/RESULTS.md`).
- ☑ `ARCHITECTURE.md`, `SPEC.md`, `DECISIONS.md`, `CHANGELOG.md` complete and current; `SECURITY.md` + `docs/SECURITY-REVIEW.md` added.
- ☑ A second engine could slot behind `engine` without changing `runtime`, verified by review, with leak points documented (D3a).
- ◐ Outstanding before a `1.0`: fuzzing + sanitizer CI (need nightly), an external security review, and the `rsa` Marvin advisory (SECURITY.md).
