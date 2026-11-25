# Fuzz targets

Coverage-guided fuzzing for the parsers that read untrusted bytes (SPEC §5).

```sh
cargo +nightly fuzz list
cargo +nightly fuzz run url fuzz/corpus/url fuzz/seeds/url -- -max_total_time=60
```

The first directory is where new coverage-increasing inputs are written (it is
gitignored); the later one is read-only seeds. CI runs each target for 60
seconds this way — a smoke run that proves the targets build and that no
committed seed crashes. A real campaign belongs on dedicated infrastructure.

## What is covered

| Target | Surface |
| --- | --- |
| `url` | URL parsing, component read-back (the UTF-16 offset arithmetic), and the setter path |
| `encoding` | `TextDecoder` over every label, lossy and fatal, one-shot and split across chunks |
| `urlpattern` | URLPattern constructor strings — the path-to-regexp lexer and the component split |
| `compression` | Decompression of arbitrary bytes, chunked as a `DecompressionStream` feeds it |
| `serialization` | XML parsed into the runtime's value tree (recursive descent) |
| `yaml` | The YAML reader, and a write-and-reread of whatever it accepts |
| `keys` | The hand-written RFC 8410 key DER parsers, and `atob` |
| `bson` | BSON validation and the element walk the engine's decoder makes, and Decimal128 text, parsed and printed back |
| `dns` | DNS responses as `resolveDns` reads them: counts, record lengths, compressed-name pointers, and each record type's layout |
| `ops` | Every op, called with argument lists the input describes — numbers at every edge, BigInts, lone surrogates, huge strings, detached and shared buffers, cycles, deep nesting, throwing getters and proxies. The finding is a Rust panic under an op (`crates/runtime/src/op_fuzz.rs`) |

`ops` is the one target with a live isolate. Standing V8 up per input would cut
the rate by orders of magnitude and mostly fuzz V8, so one runtime serves 256
inputs before it is rebuilt — about 1,900 inputs a second here. Every
capability is granted and no provider that reaches the host is installed, so a
gated op gets through its argument checks and finds nothing to act on; the few
ops that act on the fuzzing process itself are skipped by name. It runs without
leak detection, which V8's own allocations defeat. The same harness runs a
seeded few thousand calls in the runtime's ordinary tests
(`no_op_panics_on_arbitrary_arguments`). The streams are still absent, covered
by the conformance suite and the Rust tests.

## Seeds

`seeds/` is committed and small. It holds a representative input per target and
**every input that has found a bug** — an input that once crashed is a
regression test, and starting from it means the fuzzer does not have to
rediscover it. `seeds/urlpattern/crash-*` are the two that found the
bracket-depth underflow in `urlpattern` 0.6.0.

Targets reach the runtime's internals through `es-runtime`'s `fuzzing` feature
(`crates/runtime/src/fuzz.rs`), which exists for this and is not public API — except `dns`, which reaches the resolver through
`es-runtime-default-providers`'s `fuzzing` feature, for the same reason.
