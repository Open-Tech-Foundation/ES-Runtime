# Changelog

All notable changes to ES-Runtime are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the project is
pre-`0.1.0` and the public API is unstable.

## [Unreleased]

### Phase 1 — Foundation

Workspace, error model, observability, CI, and a V8 engine that runs `1 + 1`
end-to-end with snapshot scaffolding (SPEC.md §6.1).

#### Added

- **Cargo workspace** (`resolver = "3"`, edition 2024, MSRV 1.95) with the
  dependency direction from ARCHITECTURE.md §2 enforced by the crate graph
  (DECISIONS.md D11). Phase 1 introduces the first two crates; the rest land in
  their own phases.
- **`es-runtime-common`** — cross-cutting primitives, no I/O, no `unsafe`
  (`#![forbid(unsafe_code)]`):
  - Error model (DECISIONS.md D12): `ExceptionClass` JS-exception taxonomy, the
    `IntoException` trait each layer implements, the `common`-layer `Error`, and
    a `Result` alias.
  - `CapabilitySet` / `Capability` — deny-by-default capability tokens
    (DECISIONS.md D7); the empty set is the default.
  - `Limits` — per-isolate resource ceilings (heap, stack depth, pending ops)
    with validation and builder setters.
  - `telemetry::init_tracing` — idempotent `tracing` subscriber install
    (ARCHITECTURE.md §8).
- **`es-runtime-engine`** — the only crate using the `v8` crate (DECISIONS.md
  D2/D3):
  - One-time V8 platform init; `Engine` owning an isolate + a persistent
    context.
  - `Engine::eval` compiles and runs source under a `TryCatch`, marshaling JS
    primitives to `Value` and mapping failures to typed `Compile` / `Execution`
    errors — no panic crosses the boundary.
  - `snapshot::build` / `Engine::with_snapshot` — startup-snapshot build/load
    scaffolding (DECISIONS.md D8), proven by a prelude-state round-trip test.
  - The isolate heap ceiling from `Limits` is installed on creation.
- **CI** (`.github/workflows/ci.yml`) — all gates from SPEC.md §5: `fmt`,
  `clippy -D warnings`, `test`, `cargo-deny`, `cargo-audit`, and an MSRV (1.95)
  build.
- **Supply-chain config** — `deny.toml` with an Apache-2.0-compatible permissive
  license allowlist; `rust-toolchain.toml`, `rustfmt.toml`. One documented
  advisory ignore: `RUSTSEC-2024-0436` (`paste` unmaintained — informational
  only, reaches us transitively through `v8`, no fix available).

#### Decisions

- **D10 — License: Apache-2.0** locked (superseding the earlier AGPL-3.0 lean),
  matching the `LICENSE`/`NOTICE` already in the repo.
- **D3a** leak points recorded for the engine boundary (see DECISIONS.md):
  uncaught-exception JS class not yet preserved; primitive-only value marshaling;
  snapshot-creation concurrency constraint.

### Phase 2 will add

The op system and the driven event loop: sync/async ops, promise resolution, a
microtask checkpoint, the tick/poll API, and timer plumbing — plus extraction of
the engine *trait* (the abstraction `runtime` will depend on) now that there will
be a second consumer to design it against.
