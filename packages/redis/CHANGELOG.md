# Changelog for `@opentf/esrun-redis`

All notable changes to **`@opentf/esrun-redis`**, the Redis client and
`runtime:db` driver for ES Runtime, are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

This package is versioned **separately from `esrun`**: it is an ordinary npm
package written entirely in JavaScript over `runtime:net`, and it moves at the
pace of the RESP protocol rather than the runtime's. What it depends on from the
runtime is `runtime:db`'s driver contract, and the `engines.esrun` range in
`package.json` states which versions of that contract it speaks. See the root
[CHANGELOG.md](../../CHANGELOG.md) for the runtime itself.

## [Unreleased]

## [0.1.0] - 2026-08-15

### Added

- **A Redis driver — the first `runtime:db` backend that never speaks SQL.** One
  connection carries two vocabularies: the portable `runtime:db` surface, and
  Redis's own commands as methods. Redis is not a SQL database and the driver
  says so rather than pretending, so the operations that have no meaning here
  fail with a portable error naming what to call instead.

  ```js
  import { connect } from "runtime:db";
  import { driver } from "@opentf/esrun-redis";

  const r = await connect("redis://localhost:6379", { driver });
  ```

- **RESP3**, negotiated by `HELLO` and falling back to RESP2, so maps arrive as
  maps and push messages are distinguishable from replies by frame type rather
  than by content.

- **Pub/sub** as a subscription, on a connection that gives itself over to it,
  over both protocols.

- **Pipelining**, and an `executeMany` that is finally worth calling.

- **`MULTI`/`EXEC`, including `WATCH`** — which is deliberately not what
  `transaction(fn)` means, and is exposed under its own name for that reason.

- **Blocking commands**, with the connection cost made explicit, plus scan
  iterators, pop counts, and a timeout that is honest about what it costs.

- **Command families beyond the basics**: streams and consumer groups, geo,
  bitmaps, HyperLogLog, hash-field TTLs (Redis 7.4+), and the range commands.
  Stream entries come back as `{ id, fields }` rather than nested arrays;
  `hexpire` and friends answer per field with Redis's own numbers instead of
  flattening four outcomes into a boolean. Anything without a helper is one
  `r.call([...])` away.

- **Cluster support**, built on the idea that redirects are the contract:
  `MOVED`/`ASK` are followed and the slot map is refreshed from them, with
  `CROSSSLOT` reported as itself so hash tags are the fix.

- **Sentinel**, with a failover that does not close your connection.

- **Reconnection**, and a careful list of what must not come back with it —
  a subscription, a `WATCH`, and an open `MULTI` are connection state, not
  client state.

- **Command timeouts and pooling.**

- **TLS, including a private certificate authority.**

### Fixed

- A blocking command with no timeout no longer keeps the connection forever
  without giving it back.
- The cluster client's `query()` returns the portable row type, like every other
  path.

[Unreleased]: https://github.com/Open-Tech-Foundation/ES-Runtime/commits/main/packages/redis
