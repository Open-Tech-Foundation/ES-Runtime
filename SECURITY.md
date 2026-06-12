# Security

ES-Runtime is a security-hardened, embeddable JavaScript runtime (Layer A). This
document records the project's security posture and any **known, accepted gaps**
that are tracked for revisit. Architectural guarantees are specified in
`docs/SPEC.md` §4 and the rationale in `docs/DECISIONS.md`.

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
