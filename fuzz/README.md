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
| `msgpack` | The MessagePack reader and writer round-tripped, and the fast path's validator held to agreeing with the reader |
| `db_wire` | The PostgreSQL, MySQL and MongoDB drivers answered by the input: handshake, authentication (SCRAM, `caching_sha2` and its RSA key exchange), framing, and every value decoder in Rust and JavaScript (`crates/runtime/src/script_fuzz.rs`) |
| `request_parsers` | What a server parses out of a request, in JavaScript: `formData()` (multipart and urlencoded), `Headers`, `CookieMap` and `Cookie.parse`, `URLSearchParams` and a body's content type |
| `ops` | Every op, called with argument lists the input describes — numbers at every edge, BigInts, lone surrogates, huge strings, detached and shared buffers, cycles, deep nesting, throwing getters and proxies. The finding is a Rust panic under an op (`crates/runtime/src/op_fuzz.rs`) |

`ops`, `db_wire` and `request_parsers` run a live isolate. Standing V8 up per input would cut
the rate by orders of magnitude and mostly fuzz V8, so one runtime serves 256
inputs before it is rebuilt — about 1,900 inputs a second here. Every
capability is granted and no provider that reaches the host is installed, so a
gated op gets through its argument checks and finds nothing to act on; the few
ops that act on the fuzzing process itself are skipped by name. It runs without
leak detection, which V8's own allocations defeat. The same harness runs a
seeded few thousand calls in the runtime's ordinary tests
(`no_op_panics_on_arbitrary_arguments`). The streams are still absent, covered
by the conformance suite and the Rust tests.

`db_wire` and `request_parsers` (`crates/runtime/src/script_fuzz.rs`) reach what
neither the parsers nor `ops` do: code that reads untrusted bytes in
JavaScript, and Rust state machines that only run behind a socket. `db_wire`
gives the runtime a network whose every read returns the next piece of the
input (a length byte, then that many bytes; 0 is the rest) and whose writes go
nowhere, then connects each driver and runs a query. Besides a Rust panic, both
report an error the code throws only when it has itself gone wrong — a
`ReferenceError`, a blown stack, a `TypeError` from reading off `undefined` —
since a driver or parser should refuse bad input with its own error. Each has a
seeded run in the runtime's tests.

### Recording database seeds

Noise never gets a driver past its handshake, so `seeds/db_wire/` holds whole
sessions recorded from real servers. `record-db/record.py` proxies a server and
keeps what it sends, read by read; `record-db/client.mjs` is the harness's own
connect and query. With the servers in containers:

```sh
docker run -d --rm --name fz-pg -e POSTGRES_HOST_AUTH_METHOD=trust -e POSTGRES_USER=fuzz -e POSTGRES_DB=app -p 127.0.0.1:25432:5432 postgres:18.6
docker run -d --rm --name fz-my -e MYSQL_ROOT_PASSWORD=root -e MYSQL_USER=fuzz -e MYSQL_PASSWORD=secret -e MYSQL_DATABASE=app -p 127.0.0.1:23306:3306 mysql:8.4
docker run -d --rm --name fz-mongo -p 127.0.0.1:27117:27017 mongo:8
cd fuzz/record-db
for spec in "postgres-disable 0 25499 25432" "postgres-prefer 1 25498 25432" "mysql 2 25497 23306" "mongodb 3 25496 27117"; do
  set -- $spec
  python3 -I record.py $1 $2 $3 $4 -- esrun --allow-net=127.0.0.1 --allow-env client.mjs $2 $3
done
```

Postgres runs with `trust` so a replayed session needs no fresh SCRAM nonce.
`the_seeds_are_sessions_that_succeed` checks that every recorded seed still
gets its driver through the query; rerecord when it fails after a driver
change.

## Seeds

`seeds/` is committed and small. It holds a representative input per target and
**every input that has found a bug** — an input that once crashed is a
regression test, and starting from it means the fuzzer does not have to
rediscover it. `seeds/urlpattern/crash-*` are the two that found the
bracket-depth underflow in `urlpattern` 0.6.0, and `seeds/ops/oom-*` the two that
made `random_bytes` allocate 4 GB (D256).

Targets reach the runtime's internals through `es-runtime`'s `fuzzing` feature
(`crates/runtime/src/fuzz.rs`), which exists for this and is not public API — except `dns`, which reaches the resolver through
`es-runtime-default-providers`'s `fuzzing` feature, for the same reason.
