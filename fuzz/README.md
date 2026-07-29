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
| `keys` | The hand-written RFC 8410 key DER parsers, and `atob` |

The JS↔Rust marshaler and the streams are deliberately absent: they need a live
isolate, and standing V8 up per iteration would cut the iteration rate by orders
of magnitude and mostly fuzz V8. They are covered by the conformance suite and
the Rust tests instead.

## Seeds

`seeds/` is committed and small. It holds a representative input per target and
**every input that has found a bug** — an input that once crashed is a
regression test, and starting from it means the fuzzer does not have to
rediscover it. `seeds/urlpattern/crash-*` are the two that found the
bracket-depth underflow in `urlpattern` 0.6.0.

Targets reach the runtime's internals through `es-runtime`'s `fuzzing` feature
(`crates/runtime/src/fuzz.rs`), which exists for this and is not public API.
