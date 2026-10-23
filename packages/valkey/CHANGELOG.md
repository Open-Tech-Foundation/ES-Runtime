# Changelog for `@opentf/esrun-valkey`

All notable changes to **`@opentf/esrun-valkey`**, the Valkey client and
`runtime:db` driver for ES Runtime, are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

This package is versioned **separately from `esrun`**: it is an ordinary npm
package written entirely in JavaScript over `runtime:net`, built on
[`@opentf/esrun-redis`](../redis/CHANGELOG.md). See the root
[CHANGELOG.md](../../CHANGELOG.md) for the runtime itself.

## [Unreleased]

### Added

- The `@opentf/esrun-valkey` package: a Valkey client and `runtime:db` driver,
  built on `@opentf/esrun-redis`'s protocol under Valkey's names (DECISIONS
  D185). `driver`, `valkeyCluster` and `valkeySentinel` take `valkey://` and
  `valkeys://` (TLS), and `redis://` and `rediss://` beside them; what they
  open reports `backend` as `valkey` and its errors say so. The classes are
  exported as `ValkeyConnection`, `ValkeyPooled` and `ValkeyCluster`. Tested
  against Valkey 8 and 9.
