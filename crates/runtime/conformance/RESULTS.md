# Conformance results

A curated in-repo conformance suite for the **implemented** WinterTC / Minimum
Common Web API surface. Each `conformance/*.js` file is a set of spec-behaviour
assertions run by the `conformance_suite_passes` test (in `crates/runtime`),
which is part of `cargo test` and therefore a CI gate. The recorded count below
is enforced as a non-regression floor (`BASELINE` in that test).

This is **not** the full Web Platform Tests harness (no `testharness.js`); it is
a focused, gateable suite over the surface we actually ship, and it is meant to
**trend up** as coverage and the implemented surface grow.

## Snapshot

| | |
| --- | --- |
| Assertions passing | **62 / 62** (100%) |
| Files | 9 |
| Last updated | 2026-06-12 |

### Coverage by file

| File | Area (SPEC §) | Assertions |
| --- | --- | --- |
| `encoding.js` | TextEncoder/TextDecoder §2.3 | 7 |
| `base64.js` | atob/btoa §2.3 | 6 |
| `url.js` | URL/URLSearchParams §2.4 | 7 |
| `structured-clone.js` | structuredClone §2.1 | 6 |
| `events.js` | Event/EventTarget §2.7 | 7 |
| `abort.js` | AbortController/Signal §2.6 | 6 |
| `crypto.js` | crypto/subtle §2.10 | 8 |
| `streams.js` | Readable/Writable/Transform + byte/BYOB §2.8 | 11 |
| `performance.js` | performance, microtasks §2.11/§2.1 | 4 |

## Not yet covered

Deferred surface (tracked in SPEC §7), so deliberately untested here: streaming
`fetch` request bodies, `URLPattern`, and the asymmetric
`crypto.subtle` JWK edge cases. `fetch` itself is exercised by the Rust
integration tests (it needs a mock transport, not available in this pure-JS
harness). Adding assertions here as features land is how the pass count grows.
