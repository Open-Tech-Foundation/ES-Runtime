# Changelog for `@opentf/esrun-mongodb`

All notable changes to **`@opentf/esrun-mongodb`**, the MongoDB driver for ES
Runtime, are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

The driver itself is built into `esrun` (`mongodb` in `runtime:db`); this
package re-exports it with its TypeScript types and the BSON classes, so its
releases follow the runtime's. See the root [CHANGELOG.md](../../CHANGELOG.md)
for the runtime itself.

## [Unreleased]

## [0.1.0] - 2026-10-27

### Added

- The package: `driver` (the runtime's `mongodb` driver), the BSON classes
  (`ObjectId`, `Decimal128`, `UUID`, `Timestamp`, `Binary`, `Code`,
  `BSONRegExp`, `MinKey`, `MaxKey`, `Double`) and the collection API's types,
  `MongoSession`, `MongoTransaction` and `TransactionOptions` included.
  Needs esrun 0.44 or later.
