# ARCHITECTURE

Embeddable JavaScript runtime built on V8, exposing the **WinterTC Minimum Common Web API**, with **all I/O injectable**. Written in Rust, from scratch. This document describes *how* the system is built; see `SPEC.md` for *what* it implements and `DECISIONS.md` for *why*.

This is **Layer A**. A future actor-model VM (**Layer B**, separate repo) will embed this runtime by supplying its scheduler as the I/O provider. Every boundary here is designed for that future embedding, but no VM/process/scheduler/supervisor logic exists in this repo.

---

## 1. Design principles

1. **The runtime owns no I/O.** Network, clock, entropy, timers, filesystem — all arrive through injectable provider traits. The runtime carries no ambient authority.
2. **The runtime is driven, not self-running.** An external caller owns the loop and ticks the runtime. The runtime never spawns a thread or an event loop of its own.
3. **One engine, behind a boundary.** All V8-specific code is confined to a single crate. The API layer depends on an engine *abstraction*, so a second engine can be added later.
4. **Deny by default.** Every side-effecting operation is capability-gated. No grant, no effect.
5. **Hostile input assumed.** Executed JS may be adversarial. Every value crossing JS→Rust is validated; a runaway or malicious script is contained, never able to hang or crash the host.

---

## 2. Layered crate structure

Strict dependency direction — each crate depends only on those below it.

| Crate | Responsibility |
|---|---|
| `common` | Cross-cutting: error types, result aliases, tracing setup, byte/value helpers, capability tokens, config primitives. |
| `engine` | Engine abstraction trait **+ the V8 implementation**. The *only* crate that uses the `v8` crate. Owns isolate/context lifecycle, value marshaling, op-registration hooks, execution control (interrupt/terminate, heap-limit callback), module instantiation, snapshot build/load. |
| `providers` | I/O provider **traits only** — no concrete implementations. Plus capability/permission traits. |
| `runtime` | The WinterTC Minimum Common Web API, built on `engine` (abstraction) + `providers` (traits). Contains the JS prelude and host-call wiring. **No direct `v8` dependency.** |
| `default-providers` | Reference tokio-backed implementation of every provider trait. The **only** crate that owns a real loop / real sockets / real clock. For standalone use and tests. |
| `runtime-cli` | Thin binary wiring `runtime` + `default-providers`; runs a JS file or REPL. Proves standalone end-to-end operation. |

```
runtime-cli ──▶ runtime ──▶ engine ──▶ [v8 crate]
     │             │  └────▶ providers
     └──▶ default-providers ──▶ providers
                                  ▲
                            common (used by all)
```

---

## 3. The engine boundary

`engine` exposes an abstraction sufficient for `runtime` to never name a V8 type:

- **Lifecycle:** create/dispose isolates and contexts; build and load startup snapshots.
- **Execution control:** run scripts/modules; interrupt and terminate execution; install a near-heap-limit callback; stack-depth guard.
- **Value marshaling:** create/read primitives, objects, arrays, functions, promises, exceptions; **zero-copy** `ArrayBuffer`/typed-array transfer.
- **Op registration:** register Rust handlers callable from JS (see §4).
- **Module instantiation:** ☑ implemented. Compile/instantiate/evaluate ES modules behind an opaque `ModuleId` (no V8 type crosses): `runtime` runs the async load phase, the engine's synchronous resolve callback is a pure lookup over the compiled id map, and evaluation (top-level await) settles on the driven loop. `import.meta.url` is set by an isolate-level initializer. **Dynamic `import()`** rides a host callback whose promise the runtime settles after loading + evaluating the requested graph (shared with static imports via the realm module map). Loading is routed through the capability-checked `ModuleLoader` provider (§6). See DECISIONS D21/D22/D23.

**Honest boundary note.** Fully hiding V8 is hard — handle/scope semantics and the value API are large and leak easily. The boundary is drawn pragmatically at the points above. Where it must stay leaky, the leak is **named explicitly in `DECISIONS.md`**, not smeared into `runtime`. The test of success: a second engine could be slotted behind `engine` without editing `runtime`.

---

## 4. Op system (built from scratch)

The bridge between JS and Rust host functionality.

- **Registration:** a mechanism mapping JS-callable names to Rust handlers, installed into a context.
- **Sync ops:** execute and return immediately.
- **Async ops:** return a JS `Promise`; the underlying Rust future is tracked as pending work and resolves the promise at the next microtask checkpoint.
- **Typed, validated marshaling:** every argument from JS is treated as untrusted — lengths, ranges, and encodings are bounds-checked before use.
- **Capability check first:** an op performing a side effect verifies the relevant capability *before* dispatch; absence yields a clean JS exception, never a partial effect.

Pure-JS API surface (e.g. `URL`, `TextEncoder`) is shipped as **prelude JS baked into a V8 startup snapshot**; only world-touching behavior becomes an op.

---

## 5. Event loop — drivable, not owning

The runtime exposes a tick/poll API; the embedder decides when to advance it. One tick advances, in order:

1. due **timers** (from the `Timers`/`Clock` providers),
2. ready **async-op** resolutions,
3. a **microtask checkpoint**,
4. **unhandled-rejection** processing.

The API reports whether work remains so the embedder can park when idle. `default-providers` drives ticking on tokio for standalone use. **`runtime` never spawns a loop** — this is the exact seam Layer B replaces with its scheduler.

---

## 6. I/O providers (the integration seam)

Traits in `providers/`; concrete impls only in `default-providers/` (or, later, Layer B).

| Provider | Backs |
|---|---|
| `Clock` | `performance.now`, timers, monotonic/wall time |
| `Entropy` | `crypto.getRandomValues`, `randomUUID` |
| `Timers` | `setTimeout`/`setInterval` family |
| `NetTransport` | `fetch` (connect, send, stream response) |
| `ModuleLoader` | ES module `resolve` + `load` (both async — resolution may walk `node_modules` and read `package.json`); capability-gated on `FileSystem`. `NodeModuleLoader` serves local `file:` modules **and** `node_modules` ESM packages (D22); `FsModuleLoader` is the strict file-only loader; a deny-all default refuses imports |
| `FileSystem` | capability-scoped, async, optional/deniable |
| `TaskSpawner` | offloading blocking work at the provider's discretion |

Every provider call is async-friendly, cancellable, capability-checked, and returns typed errors. Because clock and entropy are providers, runs are **fully reproducible** under a deterministic provider set.

---

## 7. Security architecture

- **Capabilities:** deny-by-default tokens threaded from the embedder; no global escape hatches.
- **Resource limits enforced by the runtime:** per-isolate heap limit (near-heap-limit callback → graceful termination, never host OOM); execution-time/CPU watchdog (interrupt + `TerminateExecution`); stack-depth guard; bounded pending-op concurrency.
- **No panic across FFI:** boundaries wrapped in `catch_unwind`; all Rust errors converted to proper JS exception classes. A Rust panic must never unwind into V8.
- **Intrinsic integrity:** the security boundary is in Rust, not JS — the op table and capability set live in the engine's `OpState`, so guest tampering (prototype pollution, global reassignment, forging `__ops`) cannot escalate privilege. JS-surface defense-in-depth (`harden.js`) locks the `__ops` binding and freezes namespace objects; SES-style primordial freezing is left to the embedder. See `SECURITY.md` / `docs/SECURITY-REVIEW.md`.
- **Crypto:** vetted, constant-time library only; correct nonce/IV handling; no hand-rolled primitives.
- **Memory safety:** `unsafe` minimized, centralized in `engine`, every invariant documented; no handle outlives its scope.
- **Supply chain:** pinned deps; `cargo-deny` + `cargo-audit` in CI.

---

## 8. Observability & errors

- Structured `tracing` spans around ops and the loop; metrics hooks (op counts/durations, heap usage, pending ops) exposed for the embedder. No `println!`.
- One typed error enum per layer, mapped cleanly to JS exception classes (`TypeError`, `RangeError`, `DOMException`, …). Errors are never swallowed.

---

## 9. Performance

- **V8 startup snapshot** with the prelude + op shells baked in — ☑ implemented (D8); ~2.3× faster runtime construction. The `esrun` CLI builds this snapshot at compile time (`runtime-cli/build.rs`) and `include_bytes!`s it into the binary, so every launch restores the prelude rather than recompiling it (host-arch builds only; cross-compilation would need a target-run step).
- Zero-copy for `ArrayBuffer`/typed-array transfer — **audited and deferred** (D3a Phase 8): `Value::Bytes` copies on the JS→Rust crossing; sound zero-copy there needs a backing-store detach/pin protocol since async ops outlive the call scope. The Rust→JS direction **is** zero-copy: a returned `Value::Bytes` vec *moves* into the `ArrayBuffer` backing store, and op results are consumed, not cloned. Avoid gratuitous UTF-8↔UTF-16 round-trips.
- **Hot ops return offsets, not structures.** `url_parse` returns the canonical href plus component offsets (one string + 15 numbers in a JS array); the prelude's getters are lazy `slice()`s. This beat both per-property V8 object building *and* a JSON round-trip (see `bench/README.md`).

---

## 10. Data flow (standalone `fetch` example)

```
JS: fetch(url)
  └▶ fetch prelude builds Request, calls async op `op_fetch`
       └▶ runtime: capability check (net) → marshal args
            └▶ NetTransport provider (default-providers/tokio): connect, send
                 └▶ response streamed back as a ReadableStream (byte stream)
       └▶ promise resolved at microtask checkpoint → JS Response
```

Layer B later replaces the `NetTransport` impl with its scheduler-routed transport — `runtime` is unchanged.

## 11. Data flow (ES module graph)

Because V8 resolves a graph synchronously, the whole graph is loaded *before* instantiation:

```
runtime.load_module_source(entry, src, loader)
  └▶ engine.compile_module(entry) → ModuleId           (parse; no code runs)
  └▶ loop: engine.module_requests(id) → ["./util.mjs", …]
       └▶ ModuleLoader.resolve(spec, referrer) → canonical file: id   (async; capability check: FileSystem; bare specifiers walk node_modules)
       └▶ ModuleLoader.load(id) → source (async)        (dedup by id: diamonds/cycles load once)
       └▶ engine.compile_module(id, source) → ModuleId
  └▶ engine.instantiate_module(entry, resolved)         (sync resolve callback = pure id lookup)
  └▶ engine.evaluate_module(entry)                      (returns a promise; top-level await)
       └▶ driven loop ticks until module_eval_state() settles (Completed / Failed)
```

Layer B can supply its own `ModuleLoader` (e.g. scheduler-routed or content-addressed) — `runtime` and `engine` are unchanged.

**Dynamic `import()`** runs the same loader, but *during* evaluation: V8's host-import callback records the request and hands back a promise; the runtime's `process_dynamic_imports()` (driven each loop iteration, since loading is async) resolves + loads + instantiates the graph — deduped against the realm module map, so it shares instances with static imports — then settles the promise with the module namespace once the module's own evaluation (incl. top-level await) completes.
