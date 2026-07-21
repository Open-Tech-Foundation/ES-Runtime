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
| Assertions passing | **86 / 86** (100%) |
| Files | 15 |
| Last updated | 2026-07-21 |

### Coverage by file

| File | Area (SPEC §) | Assertions |
| --- | --- | --- |
| `encoding.js` | TextEncoder/TextDecoder §2.3 | 7 |
| `base64.js` | atob/btoa §2.3 | 6 |
| `url.js` | URL/URLSearchParams §2.4 | 9 |
| `structured-clone.js` | structuredClone §2.1 | 6 |
| `events.js` | Event/EventTarget §2.7 | 7 |
| `abort.js` | AbortController/Signal §2.6 | 6 |
| `crypto.js` | crypto/subtle §2.10 | 8 |
| `streams.js` | Readable/Writable/Transform + byte/BYOB §2.8 | 11 |
| `performance.js` | performance, microtasks §2.11/§2.1 | 4 |
| `exceptions.js` | DOMException / error classes §2.1 | 4 |
| `wasm.js` | WebAssembly JS API | 18 |

### Files present but not counted

`protobuf.js`, `serialization.js`, `serialization_edge.js` and `jsonl_test.js`
load and run, but every assertion in them is `async`. This harness settles the
async queue by ticking the runtime directly rather than through a driver, and
those tests await work it does not advance, so they contribute **0** to the count
above — uncounted, not failing. Verify them under `esrun`, not by this number.

That limitation is why `wasm.js` asserts only the synchronous WebAssembly API
plus the streaming paths that reject before reaching V8. The resolving async
paths (`compile`, `instantiate`, `compileStreaming`, `instantiateStreaming`)
depend on the driver pumping V8's foreground task queue, so they are verified
under `esrun` instead.

## Not yet covered

Deferred surface (tracked in SPEC §7) is deliberately untested here. The pure-JS
pending items are: `reportError` → global `ErrorEvent` dispatch (§2.1), AES-CTR
counter widths other than 32/64/128 bits (§2.10), and RSA-OAEP non-UTF-8 labels
(§2.10). Surface that needs host I/O — streaming `fetch` request bodies, the
WebSocket and `node_modules` edges — is covered (where covered at all) by the
Rust integration tests; `fetch` itself is exercised there too (it needs a mock
transport, not available in this pure-JS harness). Adding assertions here as
features land is how the pass count grows.
