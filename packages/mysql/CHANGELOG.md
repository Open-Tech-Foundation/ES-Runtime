# Changelog for `@opentf/esrun-mysql`

All notable changes to **`@opentf/esrun-mysql`**, the MySQL and MariaDB driver
for ES Runtime, are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

This package is versioned **separately from `esrun`**: it is an ordinary npm
package written entirely in JavaScript over `runtime:net`, and it moves at the
pace of the MySQL protocol rather than the runtime's. See the root
[CHANGELOG.md](../../CHANGELOG.md) for the runtime itself.

## [Unreleased]

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
