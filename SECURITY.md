# Security

ES-Runtime is a security-hardened, embeddable JavaScript runtime (Layer A). This
document records the project's security posture and any **known, accepted gaps**
that are tracked for revisit. Architectural guarantees are specified in
`docs/SPEC.md` §4 and the rationale in `docs/DECISIONS.md`. A full threat model,
attack-surface enumeration, and residual-risk register is in
[`docs/SECURITY-REVIEW.md`](docs/SECURITY-REVIEW.md).

## Reporting

Until a formal channel is published, report suspected vulnerabilities privately
to the maintainer rather than via public issues.

## Runtime safety status

The resource-limit / FFI-safety spine (SPEC.md §4) is in place as of Phase 9:

- **Heap limit** — a near-limit guard terminates execution before the host OOMs.
- **Execution watchdog** — a thread-safe `InterruptHandle` terminates a runaway
  script; it surfaces as `Error::Terminated`, never a hang (`esrun --timeout`).
- **Stack guard** — V8-native; deep recursion is a catchable `RangeError`.
- **Bounded pending-ops** — adversarial JS can't pile up unbounded host work.
- **Panic containment** — op/timer/reject callbacks are `catch_unwind`-wrapped,
  so a host panic is a JS exception, not an unwind across the FFI (assumes
  `panic = "unwind"`).
- **Deny-by-default capabilities**; deterministic providers for reproducibility.

**Not yet hardened** (later Phase 9): `cargo-fuzz` (URL/streams/encoding/the
marshaler), sanitizer CI (Miri/ASAN), a WPT/min-common conformance run, a
systematic intrinsic-integrity (prototype-pollution) audit, and an external
security review. **Until those land, do not run hostile/untrusted code** with
`esrun` (which also grants all capabilities); the embeddable library lets an
embedder restrict capabilities and inject its own providers.

## Environment files & secret masking

`.env` support (DECISIONS D30) is built so that **what the guest can read from
the environment is an explicit host decision**, and so that secret values resist
**accidental** disclosure:

- **No implicit disk reads.** A single `.env` file is loaded **only** via an
  explicit `esrun --env-file <path>`. There is no auto-discovery of a `.env`
  in the working directory or project root — nothing on disk is read into the
  guest's environment unless you ask for it. This is a CLI/host feature; the
  embeddable library never loads env files and never mutates the real process
  environment (the file values are an in-memory overlay on `runtime:process`).
- **OS environment wins by default.** Loaded values fill only keys the OS does
  not already set, so a checked-in `.env` cannot silently clobber a production
  deployment's real configuration. `--env-override` opts into letting file
  values win.
- **Secret masking.** Env entries with a secret-bearing key (case-insensitive)
  — ending in `_KEY(S)`, `_TOKEN(S)`, `_SECRET(S)`, `_PASS`, `_PASSWORD(S)`, or
  containing `CREDENTIAL`/`AUTH` (as an underscore-delimited word) — are exposed
  by `runtime:process` as an opaque
  `Secret` that renders as `"[redacted]"` in `console` output, string coercion /
  template literals, and `JSON.stringify`. The real value is held in a
  module-private `WeakMap` and is obtainable only via the explicit
  `unmask(value)` helper. **Scope:** this defends against *accidental* leakage
  to logs and serialized output — it is **not** a barrier against hostile guest
  code, which can call `unmask` itself (the guest is already trusted with the
  value). Parser errors never include a variable's value.

## Intrinsic integrity (prototype pollution / global tampering)

**The security boundary is in Rust, not in JavaScript.** The op table and the
capability set live in the engine's `OpState`; every capability-gated op is
checked there before dispatch. Consequently, no amount of guest JS tampering —
polluting `Object`/`Array.prototype`, reassigning or deleting globals, or trying
to forge `globalThis.__ops` — can grant a capability or dispatch an op the host
did not register and gate. This is covered by tests (`capability_gate_survives_js_tampering`,
`op_table_binding_is_locked`, `op_dispatch_survives_prototype_pollution`).

As defense-in-depth on the JS surface, `harden.js` (the last prelude fragment)
locks the `globalThis.__ops` binding (non-writable/non-configurable, while the
object stays extensible so the host can still register ops) and freezes the
runtime's plain namespace objects (`console`; `crypto`/`performance` are frozen
at definition).

**Deliberately deferred — SES-style primordial hardening.** Freezing the JS
primordials (`Object.prototype`, `Array.prototype`, …) would protect the
*prelude's own* correctness against pollution, but it is an opinionated policy
with real guest-compatibility cost. It is left to the embedder / Layer B rather
than baked into a general-purpose Layer A. Until an embedder opts in, a guest
that pollutes primordials can break the *prelude's* JS behaviour for itself — it
still cannot escalate privilege past the Rust boundary.

## Supply-chain gates

Every change must pass `cargo deny check` and `cargo audit` in CI (`docs/SPEC.md`
§5). Advisory exceptions are never silenced globally: each is listed explicitly,
with a rationale, in **both** `deny.toml` and `.cargo/audit.toml`, and is
revisited rather than forgotten.

## Known accepted gaps (revisit list)

### RSA timing sidechannel — RUSTSEC-2023-0071 ("Marvin Attack")

- **What.** `crypto.subtle` RSA (RSASSA-PKCS1-v1_5, RSA-PSS, RSA-OAEP) is backed
  by the RustCrypto `rsa` crate (`docs/DECISIONS.md` D9). That crate carries
  RUSTSEC-2023-0071, a medium-severity (5.9) timing sidechannel in RSA
  private-key operations. **No fixed upgrade exists** — the issue is
  architectural in RustCrypto's RSA and has been open since 2023.
- **Why accepted (maintainer, 2026-06-12).** RSA private-key operations run
  **host-side**; a sandboxed guest does not get a high-resolution local timing
  oracle against them, which lowers practical exploitability. The alternatives
  were weighed and each costs more than it buys for this project:
  - **aws-lc-rs** (constant-time) draws randomness from its own internal OS
    CSPRNG with no hook for the injected `Entropy` provider — breaking the
    runtime's "no ambient authority / all I/O injected" thesis for RSA — and
    adds a C/assembly crypto backend to the otherwise pure-Rust `runtime` crate.
  - **openssl-rs** adds a system OpenSSL dependency, regressing the portable,
    self-contained build goal (SPEC §1, D2).
- **Mitigations in place.** All RSA randomness (key generation, PSS salt,
  PKCS#1 v1.5 blinding, OAEP padding) is routed through the injected `Entropy`
  provider — never ambient `OsRng` — preserving determinism under seeded
  providers and capability control. RSA is capability-gated like all of
  `crypto.subtle`.
- **Revisit when.** RustCrypto ships a constant-time RSA, or the
  `elliptic-curve` 0.14 / `digest` 0.11 generation reshapes the stack such that
  a vetted, constant-time, provider-routable backend becomes available.

### `paste` unmaintained — RUSTSEC-2024-0436

Informational (unmaintained, not a vulnerability). Reaches us only transitively
through the `v8` crate; not a direct dependency and not removable without an
upstream v8 change. Revisit when v8 drops it.
