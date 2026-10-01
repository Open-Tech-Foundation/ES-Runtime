# Changelog for `@opentf/esrun-mysql`

All notable changes to **`@opentf/esrun-mysql`**, the MySQL and MariaDB driver
for ES Runtime, are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

This package is versioned **separately from `esrun`**: it is an ordinary npm
package written entirely in JavaScript over `runtime:net`, and it moves at the
pace of the MySQL protocol rather than the runtime's. See the root
[CHANGELOG.md](../../CHANGELOG.md) for the runtime itself.

## [Unreleased]

## [0.2.1] - 2026-10-01

_Dependency updates._

## [0.2.0] - 2026-10-01

### Changed

- **The package re-exports the driver built into `runtime:db`** (esrun 0.35 and
  later; DECISIONS D147). Its protocol now runs in the runtime, in Rust, and
  `import { mysql } from "runtime:db"` is the same driver. Code that imports
  `driver` from this package keeps working unchanged, and every test in the
  package's suite passes against it, on MySQL 8.4 and MariaDB 11.
- `engines.esrun` is now `>=0.35.0`.

### Removed

- The JavaScript implementation, and with it the value exports
  `MySqlConnection`, `MySqlPooled`, `MYSQL_DIALECT`, `environmentDefaults` and
  `parseConnectionString`. The types `MySqlConnection`, `MySqlOptions`,
  `MySqlPooled`, `MySqlPoolOptions`, `MySqlRow` and `MySqlValue` are still
  exported, from `runtime:db`'s declarations.

## [0.1.3] - 2026-09-29

_Dependency updates._

## [0.1.2] - 2026-09-28

_Dependency updates._

## [0.1.1] - 2026-09-25

### Added

- **The MySQL driver.** MySQL's client/server protocol in JavaScript over
  `runtime:net`, for `mysql:` and `mariadb:` URLs: `caching_sha2_password`
  (fast, full over TLS, and full over plaintext to a pinned `serverPublicKey`
  or — opted into — a retrieved one) and
  `mysql_native_password`, verified TLS, prepared statements cached per
  connection, binary rows transcoded into `runtime:db`'s shared layout and
  decoded lazily, Temporal values for dates and times, `executeScript`,
  procedures with several result sets, payloads past 16 MiB, cancellation by
  `KILL QUERY`, a server-side `statementTimeout`, and pooling. Passes
  `runBackendConformance()` against MySQL 8.4 and MariaDB 11.
