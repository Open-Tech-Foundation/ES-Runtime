# Security review (internal)

A consolidated threat model, attack-surface enumeration, and residual-risk
register for ES-Runtime (Layer A). This is an **internal** review by the
implementers; it is **not** a substitute for an external audit (which is an
outstanding pre-`1.0` item — see §6). The user-facing posture and reporting
channel live in the top-level [`SECURITY.md`](../SECURITY.md).

Status: pre-`1.0`. The resource-limit / FFI-safety spine is implemented and
tested (SPEC §4); fuzzing, sanitizer CI, and an external review are not yet
done. **Do not run hostile or untrusted code in production yet.**

## 1. Trust model & actors

| Actor | Trust | Notes |
| --- | --- | --- |
| **Embedder / host** | Trusted | Constructs the runtime, grants capabilities, supplies providers, drives the loop. Owns the security policy. |
| **Guest JavaScript** | **Untrusted / potentially hostile** | The code the runtime executes. The whole design assumes it may be adversarial. |
| **Providers** | Host-supplied, trusted-but-fallible | Clock/Entropy/Net/Console/Timers. The host vouches for them; they may still *fail* (errors are typed) but are not treated as adversarial. |
| **V8** | Trusted, large | The JS engine. Memory-safe within its own model; the FFI surface to it is the main `unsafe` locus, confined to `engine`. |

The core property: **the security boundary is in Rust, not in JavaScript.** The
op table and the capability set live in the engine's `OpState`. Guest JS runs in
a realm it can mutate freely, but it cannot reach across the op boundary except
through host-registered, capability-gated ops.

## 2. Trust boundaries

1. **The op boundary (JS → Rust).** Guest JS calls `globalThis.__ops.<name>`,
   which dispatches (in Rust) to a host handler. Every call is checked against
   the capability set *before* the handler runs (capability-check-first).
2. **The capability gate.** Deny-by-default `CapabilitySet` in `OpState`; only
   the embedder grants capabilities. No ambient authority, no JS-reachable
   escalation path.
3. **Provider injection.** All side effects (time, randomness, network, console,
   timers) flow through injected provider traits — there is no direct OS access
   from `runtime`/`engine`. Real I/O exists only in `default-providers`.
4. **The FFI / `unsafe` surface.** All V8 interaction and all `unsafe` is
   confined to `engine` (`#![forbid(unsafe_op_in_unsafe_fn)]` workspace-wide;
   `runtime`/`common`/`providers` are `#![forbid(unsafe_code)]`).

## 3. Attack surface & defenses

| Surface | Threat | Defense | Status |
| --- | --- | --- | --- |
| Hostile guest JS | CPU exhaustion (infinite loop) | Execution watchdog (`InterruptHandle::terminate` from another thread) → `Error::Terminated` | ☑ tested |
| | Heap exhaustion (heap bomb) | Near-heap-limit callback terminates before host OOM | ☑ tested |
| | Stack exhaustion (deep recursion) | V8-native stack guard → catchable `RangeError` | ☑ tested |
| | Privilege escalation via prototype/global tampering | Boundary is in Rust; capability check in `OpState` | ☑ tested |
| | Unbounded pending async ops | `max_pending_ops` bound → `RangeError` | ☑ tested |
| The op ABI (`__ops`) | Direct raw-op calls with bad arguments | Handlers validate marshaled args, return typed `OpError`s | ☑ |
| | Forging / replacing `globalThis.__ops` | Binding locked (`harden.js`); dispatch + op-id validation in Rust | ☑ tested |
| Marshaling (`Value`) | Malformed/edge-case values from JS | Defensive marshaling; primitives + copied bytes only | ☑ |
| | A host op handler **panics** | `catch_unwind` around op/timer/reject callbacks → JS exception, not an unwind across V8 | ☑ tested (assumes `panic = "unwind"`) |
| Providers | A provider returns an error (e.g. entropy fails) | Typed `ProviderError` → JS exception; no partial effect | ☑ |
| | A provider **panics** | Contained as a host-op panic (above), unless `panic = "abort"` | ◐ |
| V8 / FFI | Use-after-free of handles/scopes | Pinned-scope API; handles never outlive their scope; isolate `!Send` | ☑ by construction |
| | Rust panic crossing into C++ | `catch_unwind` at every V8-invoked callback | ☑ |
| Supply chain | Vulnerable/unmaintained/incompatibly-licensed deps | Pinned versions; `cargo-deny` + `cargo-audit` CI gates; documented exceptions | ☑ |

## 4. Cryptography

`crypto.subtle` uses vetted RustCrypto primitives (DECISIONS D9): constant-time
HMAC verify, AEAD tag checks, no hand-rolled primitives. All asymmetric
randomness (RSA/EC key gen, PSS salt, PKCS#1 blinding, OAEP padding, ECDSA
nonces) is routed through the injected `Entropy` provider — never ambient
`OsRng` — preserving reproducibility under seeded providers and capability
control. **Carried risk:** the `rsa` crate's Marvin timing sidechannel
(RUSTSEC-2023-0071, no fix available); accepted because RSA private-key ops are
host-side. See `SECURITY.md`.

## 5. Determinism & observability

Under deterministic providers (`ManualClock`, `SeededEntropy`, …) runs are
reproducible — useful for testing and for an embedder that needs replayable
execution. Structured `tracing` spans surround ops and the loop; there is no
`println!` in library crates (lint-enforced).

## 6. Residual risks & known gaps

1. **No external security review or fuzzing yet.** `cargo-fuzz` (URL/encoding/
   streams/marshaler) and sanitizer CI (Miri on the safe core, ASAN on the FFI)
   require a nightly toolchain and are outstanding. **This is the single biggest
   reason not to run untrusted code in production yet.**
2. **`rsa` Marvin timing sidechannel** (RUSTSEC-2023-0071) — accepted, no fix
   available (SECURITY.md / DECISIONS D9).
3. **SES-style primordial hardening deferred.** A guest can pollute `Object`/
   `Array.prototype` and break the *prelude's own* JS behaviour for itself; it
   **cannot** escalate privilege past the Rust boundary. Full primordial
   freezing is an embedder/Layer-B policy, not baked into Layer A.
4. **`panic = "abort"` builds.** Panic containment assumes `panic = "unwind"`.
   Under `abort`, a host-op (or provider) panic aborts the process — the chosen
   policy for that build, but worth stating.
5. **Watchdog is wall-clock + interruption-point based.** It stops scripts at V8
   interruption points (tight loops are interruptible). It is not cycle-accurate
   CPU accounting, and a pathological non-yielding native path could delay
   termination.
6. **Side channels (Spectre, timing).** Relies on V8's own mitigations; not
   separately addressed at this layer.
7. **`esrun` grants all capabilities** (trusted-local-script mode), but module
   resolution **and** the filesystem capability are **root-jailed by default**
   (DECISIONS D25). `esrun` loads modules through `NodeModuleLoader` and serves
   `runtime:fs` from `SystemFileSystem`; both confine every *real* (canonicalized)
   path to the detected **project root** — the nearest ancestor of the entry
   containing `node_modules`/`package.json`, else the entry's directory — and
   reject a path that escapes it via `..` or a symlink whose realpath leaves the
   root (enforced by `path::within_root`; covered by jail tests in
   `default-providers` and a `../escape.txt` rejection test in `runtime-cli`).
   So a granted capability's reach is the project root, not the whole filesystem.
   Residual nuances: (a) within that root, the trusted-mode all-capabilities grant
   has full reach — `esrun` is a runner for *trusted* local code, not a sandbox
   for hostile code; (b) legitimate cross-root setups (workspaces, `pnpm link`, a
   symlinked external store) need the **relax flag** (additional allowed roots),
   which is the still-deferred CLI part of D24/D25; and (c) the strict
   `FsModuleLoader` — an embedder-only alternative that `esrun` does **not** use —
   is unjailed by design, so an embedder choosing it must add its own confinement.
   An embedder sandboxing untrusted code should still withhold `FileSystem`/`Net`
   outright rather than rely on the jail alone.
8. **Engine after `Terminated` is "spent."** The embedder should discard a
   runtime whose `eval`/tick returned `Error::Terminated` rather than reuse it.

## 7. Guidance for embedders

- **Grant the minimum capabilities** the workload needs; deny-by-default is the
  starting point. Never grant `Net`/`FileSystem` to untrusted code.
- **Set `Limits`** (heap, `max_pending_ops`) appropriately; run a **watchdog
  thread** via `Runtime::interrupt_handle()` (or `esrun --timeout`) to bound
  execution time.
- **Inject your own providers** to mediate/observe all I/O; use deterministic
  providers where reproducibility matters.
- **Build with `panic = "unwind"`** to keep panic containment effective.
- **Consider SES-style primordial hardening** in your own prelude layer if you
  run mutually-distrusting guest code in one realm.
- **Until fuzzing + an external review land, treat this as not-yet-hardened for
  hostile input.**
