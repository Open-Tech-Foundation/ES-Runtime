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
| Assertions passing | **254 / 254** (100%) |
| Known deviations (`todo`) | **0** |
| Files | 21 |
| Last updated | 2026-07-28 |

A file states spec behaviour two ways. `test(...)` is behaviour the runtime
**has** — it is counted above and gated as a non-regression floor. `todo(...)`
is behaviour the spec requires that the runtime **does not have yet**: it is
tallied separately and does not fail the build, but a `todo` that starts passing
*does* fail it, so a fix cannot land without being promoted to `test`. That makes
every known deviation an executable, self-retiring entry rather than prose.

### Coverage by file

| File | Area (SPEC §) | Passing | Deviations |
| --- | --- | --- | --- |
| `encoding.js` | TextEncoder/TextDecoder §2.3 | 16 | — |
| `base64.js` | atob/btoa §2.3 | 6 | — |
| `url.js` | URL/URLSearchParams §2.4 | 17 | — |
| `urlpattern.js` | URLPattern §2.4 | 16 | — |
| `structured-clone.js` | structuredClone §2.1 | 18 | — |
| `events.js` | Event/EventTarget, ErrorEvent, global scope §2.7 | 23 | — |
| `abort.js` | AbortController/Signal §2.6 | 8 | — |
| `crypto.js` | crypto/subtle §2.10 | 10 | — |
| `streams.js` | Readable/Writable/Transform + byte/BYOB §2.8 | 17 | — |
| `performance.js` | performance, User Timing §2.11/§2.1 | 11 | — |
| `exceptions.js` | DOMException / error classes §2.1 | 4 | — |
| `timers.js` | setTimeout/setInterval §2.5 | 3 | — |
| `blob.js` | Blob/File/FormData, object URLs §2.9 | 18 | — |
| `channel.js` | MessageChannel/MessagePort/BroadcastChannel | 8 | — |
| `fetch.js` | Headers/Request/Response object surface | 33 | — |
| `webidl.js` | Interface shape: branding, arity, iterators | 27 | — |
| `wasm.js` | WebAssembly JS API | 18 | — |

### Known deviations

**None.** Every `todo` recorded by the audit that opened this suite has been
fixed and promoted to `test`. The themes it found were: missing
`Symbol.toStringTag` branding, internal members on public prototypes, absent
argument validation, missing interface members, and outright wrong behaviour —
see the `[Unreleased]` section of [CHANGELOG.md](../../../CHANGELOG.md) for what
each turned out to be.

Two fixes are gated by Rust tests rather than here, because both need a driven
event loop or host I/O: `fetch` honouring `AbortSignal` (tested against a
transport that never responds, asserting the in-flight request future is
dropped) and `setTimeout`/`setInterval` forwarding their trailing arguments.

`todo(...)` remains available and is the right way to record the next deviation
found: it states what the spec requires, is tallied separately so it does not
fail the build, and fails the build the moment it starts passing — so a fix
cannot land without being promoted and locked in.

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
pending items are: AES-CTR
counter widths other than 32/64/128 bits (§2.10), and RSA-OAEP non-UTF-8 labels
(§2.10). Surface that needs host I/O — streaming `fetch` request bodies, the
WebSocket and `node_modules` edges — is covered (where covered at all) by the
Rust integration tests; `fetch` itself is exercised there too (it needs a mock
transport, not available in this pure-JS harness). Adding assertions here as
features land is how the pass count grows.
