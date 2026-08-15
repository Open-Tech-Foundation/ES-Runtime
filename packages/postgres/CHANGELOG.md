# Changelog for `@opentf/esrun-postgres`

All notable changes to **`@opentf/esrun-postgres`**, the PostgreSQL driver for
ES Runtime, are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

This package is versioned **separately from `esrun`**: it is an ordinary npm
package written entirely in JavaScript over `runtime:net`, and it moves at the
pace of the PostgreSQL wire protocol rather than the runtime's. What it depends
on from the runtime is `runtime:db`'s driver contract, and the `engines.esrun`
range in `package.json` states which versions of that contract it speaks. See
the root [CHANGELOG.md](../../CHANGELOG.md) for the runtime itself.

## [Unreleased]

## [0.1.0] - 2026-08-15

### Added

- **A PostgreSQL driver, and no new Rust for it.** The package's export *is* the
  driver — `connect(url, { driver })` — so adding a database to this runtime
  means adding a dependency, not adding to the runtime.

  ```js
  import { connect, sql } from "runtime:db";
  import { driver } from "@opentf/esrun-postgres";

  const db = await connect("postgres://user:secret@localhost/app", { driver });
  ```

  Everything in the `runtime:db` guide works here — the `sql` tag, streaming
  results, transactions with savepoints, `executeMany`, and the portable error
  codes. The package passes the same `runBackendConformance()` suite the
  built-in `sqlite:` backend does.

- **The capability it needs is `Net`, and nothing else.**
  `--allow-net=db.internal:5432` names the host and the port, which is narrower
  than any "may use a database" permission could be.

- **SCRAM-SHA-256 and cleartext authentication**, both over WebCrypto, with the
  server's half of SCRAM verified rather than skipped. `md5` is not implemented:
  it is deprecated upstream and the runtime has no MD5 to implement it with.

- **TLS, including a private certificate authority.** `sslmode=prefer` (the
  default) asks and continues without it, `require` fails instead, `disable`
  never asks.

- **Temporal by default.** `timestamptz` decodes to `Temporal.Instant`,
  `timestamp` to `Temporal.PlainDateTime`, `date` to `Temporal.PlainDate`,
  `time` to `Temporal.PlainTime`, `interval` to `Temporal.Duration`. A `Date`
  cannot hold a `timestamp`'s microseconds, and can only express
  `timestamp without time zone` by inventing a zone. `{ temporal: false }`
  restores `Date` and strings for code that has to hand a `Date` elsewhere.

- **Arrays**, nested and null-aware, decoded for every type in the mapping
  table. An array of a type outside it comes back as its raw literal rather than
  guessed at — the wire does not say a column is an array.

- **Binary result formats** for the columns that are cheaper to read as bytes
  (`int2` `int4` `int8` `float4` `float8` `bool` `bytea` `uuid` `timestamp`
  `timestamptz` `date`). `numeric`, `json`/`jsonb` and arrays stay text
  deliberately. A column decodes to the same JavaScript value either way, and a
  test asserts exactly that by reading the same row through both paths.

- **A connection pool**, with the release contract that makes handing a
  connection back safe.

- **Prepared statements, prepared once**, with a bound on what that caching
  costs.

- **`LISTEN`/`NOTIFY`** as a subscription, on a connection that gives itself
  over to it, plus notices and server parameters delivered as the server sends
  them rather than only in reply to a query.

- **Cancellation and timeouts.** A query can be cancelled out of band or aborted
  with an `AbortSignal`, and both connection and statement have a timeout the
  server enforces.

- **A door for scripts.** `query()`/`execute()` use the extended protocol, where
  a prepared statement is one statement by definition, so a string with two of
  them is refused with `ERR_DB_SYNTAX`; multi-statement scripts have their own
  entry point.

- **The `PG*` environment is honoured, without being required.** Options passed
  to `connect()` override the URL, which overrides the environment.

### Fixed

- A lost connection says so **once**, and keeps saying it, instead of reporting
  a different failure per call afterwards.
- A query issued during a streaming result no longer hangs the connection.

[Unreleased]: https://github.com/Open-Tech-Foundation/ES-Runtime/commits/main/packages/postgres
