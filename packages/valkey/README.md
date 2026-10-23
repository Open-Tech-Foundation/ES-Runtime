# @opentf/esrun-valkey

A [Valkey](https://valkey.io) driver for [ES Runtime](https://esrun.opentechf.org):
a `runtime:db` backend **and** a Valkey client, written entirely in JavaScript
over `runtime:net`. There is no native code in this package, and none was added
to the runtime for it.

```sh
npm install @opentf/esrun-valkey
```

```js
import { connect } from "runtime:db";
import { driver } from "@opentf/esrun-valkey";

const v = await connect("valkey://localhost", { driver });

await v.set("session:42", "ada", { ex: 3600 });
await v.get("session:42");                    // "ada"
await v.hset("user:42", { name: "ada", age: "36" });
await v.hgetall("user:42");                   // { name: "ada", age: "36" }

await v.close();
```

Valkey kept Redis's protocol, so this package is built on
[`@opentf/esrun-redis`](https://www.npmjs.com/package/@opentf/esrun-redis) rather
than beside it: the same connection, pool, pipelines, `MULTI`/`EXEC`, pub/sub,
blocking commands, cluster and Sentinel clients, under Valkey's names. The
[Redis driver's documentation](https://esrun.opentechf.org/docs/db/redis)
describes all of it; everything there applies here with `valkey` for `redis`.

## Connection strings

`valkey://[[username][:password]@]host[:port][/db][?option=value]`, and
`valkeys://` for TLS from the first byte. `redis://` and `rediss://` are taken
too, so a connection string written for Redis keeps working unchanged.

## Drivers

| Export | Opens |
| --- | --- |
| `driver` | One connection; `pool: true` makes it a pool with the same surface |
| `valkeyCluster` | A cluster client: a pool per node, routed by slot |
| `valkeySentinel` | A connection to the primary that Sentinel names |

```js
import { valkeyCluster } from "@opentf/esrun-valkey";

const cluster = await connect("valkey://10.0.0.1:7001", {
  driver: valkeyCluster,
  seeds: ["valkey://10.0.0.2:7001"],
});
```

What they open reports `backend` as `valkey` (`valkey-cluster` for a cluster),
and errors name Valkey. The classes are exported as `ValkeyConnection`,
`ValkeyPooled` and `ValkeyCluster`; they are the Redis package's own classes, so
`instanceof` holds under either name.

## Valkey-only commands

A command without a helper is one `call` away, Valkey-only ones included:

```js
await v.call(["DELIFEQ", "lock:job", token]);   // Valkey 9
```
