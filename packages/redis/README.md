# @opentf/esrun-redis

A Redis driver for [ES Runtime](https://esrun.opentechf.org) — a `runtime:db`
backend **and** a Redis client, written entirely in JavaScript over
`runtime:net`. There is no native code in this package, and none was added to
the runtime for it.

```sh
esrun add @opentf/esrun-redis
```

## Two surfaces, one connection

Most code wants the **client**, which is Redis with its own vocabulary:

```js
import { Redis } from "@opentf/esrun-redis";

const r = await Redis.connect("redis://localhost");

await r.set("session:42", "ada", { ex: 3600 });
await r.get("session:42");                    // "ada"
await r.hset("user:42", { name: "ada", age: "36" });
await r.hgetall("user:42");                   // { name: "ada", age: "36" }
await r.zadd("scores", { ada: 9.5, grace: 8 });
await r.zrange("scores", 0, -1, { withScores: true });

await r.close();
```

The **backend** is the same connection reached through `runtime:db`, for code
that is written against the portable surface rather than against Redis:

```js
import "@opentf/esrun-redis";
import { connect, queryAst } from "runtime:db";

const db = await connect("redis://localhost");
await db.execute(queryAst(["SET", "k", "v"]));

const rows = await db.query(queryAst(["LRANGE", "log", 0, -1]));
for await (const row of rows) console.log(row.value);
```

A command is an array, not a string: `queryAst(["SET", key, value])`. Nothing is
parsed and nothing is quoted, so there is no injection to prevent — the
arguments were never text that could become syntax.

## Redis is not a SQL database, and the driver says so

`runtime:db`'s contract has carried a query-AST form since its first release,
for exactly this case. This is the first backend to use it, and it declares what
it is:

| | |
| --- | --- |
| `supports.queryAst` | `true` — commands are arrays |
| `supports.sqlText` | `false` — SQL is refused with `ERR_DB_QUERY_FORM` |
| `supports.transactions` | `false` — see below |
| `supports.savepoints` | `false` |
| `supports.returning` | `false` |
| `supports.namedParameters` | `false` — Redis arguments are positional |

**`transaction()` throws `ERR_DB_UNSUPPORTED`.** Redis has `MULTI`/`EXEC`, and
it is deliberately not presented as a transaction: it queues commands and applies
them together, but a command that fails at `EXEC` time does **not** roll back the
ones beside it. A `transaction(fn)` built on it would commit half a body that
threw, which is worse than not having one. Send `MULTI` yourself through
`call()` — on a pool, inside `withConnection()` so it stays on one connection.

**`executeMany` is not atomic here**, for the same reason: there is no
transaction to wrap it in. It runs one round trip per set.

## Connection strings

```
redis://[[username][:password]@]host[:port][/db][?option=value]
rediss://…                                  TLS from the first byte
```

The path is a database **index**, not a name — `redis://host/3` is database 3.
An empty username with a password (`redis://:secret@host`) is the pre-ACL
spelling and means the `default` user.

| Option | |
| --- | --- |
| `?db=` | the database index, when the path is not used |
| `?connect_timeout=` | **seconds**, as every connection string spells it |
| `?protocol=2` \| `3` | force RESP2, or ask for RESP3 (the default) |
| `?client_name=` | `CLIENT SETNAME`, so the connection is identifiable |

A password in a query parameter is **refused**: one place for a credential means
a URL-redacting logger only has to know about one.

> Reading a connection string out of the environment? `env` redacts anything
> that looks like a secret, so a URL with a password in it arrives as
> `"[redacted]"`. Pass it through `unmask` from `runtime:process` first.

Everything is also an option: `Redis.connect(url, { password, db, tlsCa, … })`,
and explicit options beat the URL.

## TLS

Redis has no in-band upgrade — no `SSLRequest`, no plaintext phase — so
`rediss://` is an ordinary TLS socket, which makes this the one handshake here
simpler than PostgreSQL's. An internal server with a certificate from a private
authority needs that authority, because the public roots have never heard of it:

```js
const r = await Redis.connect("rediss://redis.internal", { tlsCa: await readFile("ca.crt") });
```

## RESP3, and what it buys

`HELLO 3` is sent on connect, which negotiates the protocol **and**
authenticates in one round trip. RESP3 types the reply: `HGETALL` comes back as
a map rather than a flat array the client has to know to re-pair, and a double
is a double rather than a string.

A server older than Redis 6 has no `HELLO` and one built without RESP3 answers
`NOPROTO`; both fall back to RESP2 and authenticate separately. A wrong password
is **not** a fallback — it fails, rather than quietly becoming an
unauthenticated session.

The client absorbs the difference either way: `hgetall` is an object and
`zrange({ withScores: true })` is pairs on both protocols. `r.protocol` reports
which is in force.

## Types

| Redis | JavaScript |
| --- | --- |
| bulk string | `string` (UTF-8), or `Uint8Array` with `{ binary: true }` |
| simple string | `string` |
| integer | `number`, or `bigint` past 2⁵³ |
| double (RESP3) | `number` |
| big number (RESP3) | `bigint` |
| boolean (RESP3) | `boolean` |
| null / `$-1` / `*-1` | `null` |
| array, set | `Array` |
| map (RESP3) | plain object |

Redis integers are signed 64-bit, so a counter can pass 2⁵³ — `number` where the
value is exact and `bigint` where it is not, which is the rule `runtime:db`
applies to every backend. Values are binary-safe: `{ binary: true }` on the
connection hands bulk strings back as bytes rather than decoding them.

## Rows

Through `db.query()`, a reply becomes rows by its own type — there is no table
of which command returns what, because `EVAL` returns whatever the script did:

| Reply | Rows |
| --- | --- |
| array, set | one row per element, column `value` |
| map | one row per pair, columns `field` and `value` |
| any scalar | one row, column `value` |
| null | **no rows** — `rows.first()` is `null` |

Rows are lazy views over their batch, as everywhere in `runtime:db`:
`row.toObject()` materializes one and `{ ...row }` gives an empty object. A
nested aggregate has nowhere to go in a flat row and is written as JSON text —
read `XRANGE` and friends through the client API, which returns the structure
itself.

`rows.exhausted` is always `true`. A RESP reply is complete once it has been
read, so there is no cursor to leave open and a pool gets its connection back
before the caller touches a row.

## Errors

Redis's leading word — `WRONGTYPE`, `NOAUTH`, `LOADING` — is mapped onto
`DbErrorCode`, with the original always on `e.backendCode`.

```js
import { DbErrorCode } from "runtime:db";

try { await r.set("k", "v"); }
catch (e) {
  if (e.code === DbErrorCode.AuthFailed) …   // NOAUTH, WRONGPASS, NOPERM
  if (e.backendCode === "WRONGTYPE") …       // needs Redis-specific handling
}
```

`WRONGTYPE` is deliberately **not** mapped: none of the portable codes means
"you ran a list command against a hash", and the nearest one would tell an
application something false. It stays `ERR_DB_BACKEND` with its own
`backendCode`, which is the truth.

An error reply is a complete reply, so the connection stays usable — only a
transport failure is fatal, and the first one is latched so every later caller
sees the same lost connection rather than a different symptom of it.

## Pub/sub

```js
import { Redis, createSubscriber } from "@opentf/esrun-redis";

const sub = await createSubscriber("redis://localhost");
await sub.subscribe("news", (message, { channel }) => console.log(channel, message));
await sub.psubscribe("room.*", (message, { channel, pattern }) => …);

const pub = await Redis.connect("redis://localhost");
await pub.publish("news", "hello");        // → how many subscribers received it
```

**Two connections, and that is not a workaround.** The first `subscribe` gives
its connection over to a read loop, and from then on that connection runs no
ordinary commands — `get`, `set`, even `publish` refuse with
`ERR_DB_CONNECTION_BUSY`. Over RESP2 this is the protocol's own rule, since a
subscribed connection accepts nothing but the subscribe family; over RESP3 it is
this driver's, because the read loop owns the reader and there is nobody to hand
an ordinary reply to. It is also how you would deploy it anyway: a connection
that must notice a message promptly should not be queued behind a report query.
`createSubscriber()` is `createClient()` under a name that says which one it is.

Subscribing is **confirmed** before it resolves, so publishing immediately after
cannot race it and a subscribe the server refuses fails at the call rather than
silently never firing. The loop owns reading and a `SUBSCRIBE` only needs
writing — TCP is full duplex — which is what makes that possible.

A connection given over to subscribing **stays** a subscriber. `unsubscribe()`
with no argument drops every channel and stops the messages, but not the mode.

| | |
| --- | --- |
| `subscribe(channels, handler?)` | exact channels |
| `psubscribe(patterns, handler?)` | glob patterns; the handler also gets `pattern` |
| `ssubscribe(channels, handler?)` | sharded channels (Redis 7+) |
| `unsubscribe(channels?)` | and `punsubscribe`, `sunsubscribe` |
| `onMessage` | a catch-all, after any per-channel handler |
| `onSubscribeError` | the read loop's failures, since nobody awaits it |
| `channels`, `patterns`, `shardChannels`, `subscribed` | what it is doing |

Handlers run synchronously in the read loop, so a slow one delays every later
message — hand real work to a queue. A handler that **throws** is reported to
`onSubscribeError` and the loop continues: it is the only thing reading the
socket, and letting one bad handler end it would silently stop every other
subscription on the connection.

Pub/sub is fire-and-forget. There is no queue and no delivery guarantee, and
`publish` returning `0` means nobody was listening — which is not an error.

## Blocking commands

```js
await r.blpop("queue", 5);        // → { key, value } | null
await r.brpop(["a", "b"], 5);     // the first of several that has anything
await r.blmove("src", "dst", 5);
await r.bzpopmin("scores", 5);    // → { key, member, score } | null
await r.wait(2, 1000);            // replicas acknowledged (timeout in **ms**)
```

The timeout is a **required** argument on every one of them, in the units Redis
takes it — seconds for the pop family, milliseconds for `wait`. Required rather
than defaulted, because the one value that has to be a deliberate choice is the
one meaning *forever*, and a default would make it an accident.

A blocking command holds its connection for as long as it blocks. That is
inherent: the server sends no reply until it has one, and a connection is one
conversation. So a bounded wait is a stall you chose, and everything else on
that connection waits behind it:

```js
await Promise.all([r.blpop("idle", 1), r.ping()]);   // the PING takes ~1s too
```

`0` means forever, which is not a stall but a stuck connection, and on a pooled
one it is worse than it looks — gone for the life of the process, while other
callers fail on `acquireTimeout` pointing at pool exhaustion rather than at the
cause. So it is refused unless the connection was opened to be tied up:

```js
const worker = await Redis.connect(url, { blocking: true });
await worker.call(["BLPOP", "jobs", "0"]);           // allowed here
```

`createPool` **strips** that option. A pool's premise is that its connections
come back; blocking indefinitely needs a connection of its own, by construction.

### Consuming a queue

The loop blocking pops exist for, written once:

```js
const worker = await Redis.connect(url);
for await (const job of worker.consume("jobs", { timeout: 5, signal })) {
  await handle(job.value);
}
```

It polls with a **bounded** pop even though it loops forever, and that is what
makes it interruptible: an abandoned `for await` or an aborted signal is noticed
when the current wait ends rather than never. `timeout` is the worst case for
how long stopping takes, not a latency — a job arriving mid-wait is delivered
immediately. An empty queue is not the end of the queue, so a timed-out wait
just goes round again.

Redis keeps the timeout in three different places — last for `BLPOP`, first for
`BLMPOP`, behind the `BLOCK` keyword for `XREAD` — and the check knows all
three, including that a stream legitimately named `BLOCK` is not the option.

## Pooling

```js
import { createPool } from "@opentf/esrun-redis";

const pool = createPool("redis://localhost", { max: 10 });
await pool.set("k", "v");                    // the same commands, borrowed per call
await pool.withConnection((c) => …);         // for anything stateful across commands
```

A connection goes back to the pool only if the driver vouches for it: alive, on
the database it was opened for, and not inside an open `MULTI`. A connection
left on another database by a stray `SELECT` is **destroyed** rather than handed
to the next borrower, who would otherwise find their keys pointing at a
different dataset.

## What this release does not do

Named rather than left to be discovered:

- **`MONITOR`**, which turns the connection into a firehose of every command the
  server runs. One reply per command cannot represent that; use `redis-cli`.
- **Cluster.** `MOVED` and `ASK` are reported as `ERR_DB_UNSUPPORTED` with an
  explanation rather than as a bare redirect. Connect to the node that owns the
  key, or put a proxy in front.
- **Pipelining.** Every command is one round trip, including `executeMany`. It
  is the one place this driver leaves measurable time on the table.
- **Sentinel**, and RESP3 client-side caching (server attributes are read and
  discarded).

## Tests

There is no mock. The value of this package is that it speaks a real server's
protocol, and a fake one would only ever agree with our reading of the spec.

```sh
bun run build
docker run -d --name esrun-redis-plain -p 6379:6379 redis:latest
docker run -d --name esrun-redis-auth  -p 6380:6379 redis:latest redis-server --requirepass esrun
eval "$(test/tls-server.sh)"       # optional; the tls test skips without it
./test/run.sh
```

The unit tests need no server — a wire codec does not — and cover the cases a
live server will not produce on demand: a reply split across five chunks, a
CRLF inside a bulk string, an attribute nobody asked for, RESP2's two spellings
of null, every argument position a blocking command keeps its timeout in, and
pub/sub over **both** protocols — RESP3 delivers messages as push frames and
RESP2 as ordinary arrays, so the reader tells a message from a reply by its
content there rather than by its type byte.

Both halves run in CI against three servers: a `redis:8` service container, a
password-protected one, and a TLS one with a certificate from a private
authority — because the things most likely to break are the ones a single
default server cannot show.

## License

Apache-2.0
