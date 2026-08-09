# @opentf/esrun-postgres

A PostgreSQL driver for [ES Runtime](https://es-runtime.opentechf.org) (`esrun`),
written **entirely in JavaScript** over `runtime:net`.

There is no native code in this package and none was added to the runtime for
it. That is the point of `runtime:db`: adding a database to this runtime does
not mean adding anything to the runtime.

```sh
npm install @opentf/esrun-postgres
```

```js
import "@opentf/esrun-postgres";           // registers postgres: and postgresql:
import { connect, sql } from "runtime:db";

const db = await connect("postgres://user:secret@localhost/app");

await db.execute(sql`INSERT INTO users (name) VALUES (${name})`);

for await (const user of await db.query("SELECT id, name FROM users")) {
  console.log(user.id, user.name);
}

await db.close();
```

Everything in the [`runtime:db` guide](https://es-runtime.opentechf.org/docs/db)
works here — the `sql` tag, streaming results, transactions with savepoints,
`executeMany`, and the portable error codes. This package passes the same
`runBackendConformance()` suite the built-in `sqlite:` backend does.

## Connection strings

```
postgres://user:password@host:5432/database?sslmode=require
```

| Part | Default |
| --- | --- |
| host | `localhost` |
| port | `5432` |
| user | `postgres` |
| database | the user's name |
| `sslmode` | `prefer` |

`sslmode=prefer` asks for TLS and continues without it if the server declines;
`require` fails instead; `disable` never asks. Options passed to `connect()`
override the URL.

```js
const db = await connect("postgres://localhost/app", {
  user: "app",
  password: env.PGPASSWORD,
  sslmode: "require",
});
```

## One result set at a time

A PostgreSQL connection is a single conversation, so a connection can have one
open result set. Querying while another result is still streaming is **refused**
with `ERR_DB_CONNECTION_BUSY` rather than queued:

```js
for await (const row of await db.query("SELECT * FROM big")) {
  await db.query("SELECT 1");   // ERR_DB_CONNECTION_BUSY
}
```

Queueing would deadlock rather than wait — the outer result only finishes when
the loop does, and the loop is waiting on the queue. The refusal is immediate
and says what to do instead: finish the result (`toArray()`, or let the
`for await` end), or use a second connection.

Two things this does **not** refuse. A result small enough to arrive in one
batch never holds the connection at all, so the pattern above works for small
queries. And concurrent statements with no open result set queue normally —
an exchange in flight finishes on its own, so waiting for it is finite:

```js
await Promise.all([db.execute(a), db.execute(b)]);   // fine
```

## Timeouts

```js
const db = await connect(url, {
  connectTimeout: 10_000,    // ms — the connection *and* its handshake
  statementTimeout: 30_000,  // ms — applied to every statement
});
```

| Option | URL parameter | Default |
| --- | --- | --- |
| `connectTimeout` (ms) | `connect_timeout` (**seconds**, libpq's spelling) | 10 000 |
| `statementTimeout` (ms) | `statement_timeout` (ms) | unset |

`connectTimeout` is the one that matters most: a server which completes the TCP
handshake and then says nothing is indistinguishable from a slow one, and
without a deadline that wait never ends. A *refused* connection fails on its
own; an accepted-and-ignored one does not.

`statementTimeout` is sent as a startup parameter, so the **server** enforces
it. A client-side timer cannot do this job — it would fire on a statement the
server is still running, and abandoning a connection mid-statement leaves it
unusable. The server cancels the statement, reports SQLSTATE `57014`
(`ERR_DB_TIMEOUT`), and the connection stays usable.

## When the connection dies

A transport failure is latched. Once a message has been half-read off a socket,
nothing later on it can be trusted to start on a message boundary — so the first
failure is kept and every later call is answered with the same
`ERR_DB_CONNECTION_LOST`, rather than each caller meeting a different symptom of
one dead connection: a hang, a length that makes no sense, a message tag nobody
sent.

Closing a connection that has already died is not an error and does not hang.

## TLS and private certificate authorities

`sslmode=prefer` (default) asks for TLS and continues without it if the server
declines; `require` fails instead; `disable` never asks.

An internal PostgreSQL usually presents a certificate from a **private**
authority, which the public roots have never heard of. Name it:

```js
import { file } from "runtime:fs";

const db = await connect(url, {
  sslmode: "require",
  sslRootCert: await file("/etc/ssl/internal-ca.crt").text(),
});
```

The certificate is *added* to the public roots, never swapped for them, and the
hostname and chain checks still run — a server matching neither is still
refused. There is no option to skip verification.

`sslrootcert` also works in the URL, but it takes the **certificate itself**,
not a path as libpq does: reading a file is a capability, and a connection
string should not exercise it on your behalf.

## Capabilities

The driver needs **`Net`**, and nothing else — it is an ordinary outbound TCP
connection. Scope it to the database and nothing else:

```sh
esrun --deny-all --allow-imports --allow-net=db.internal:5432 app.js
```

That is a narrower grant than a "may use a database" permission could ever be:
it names the host and the port.

## Types

| PostgreSQL | JavaScript |
| --- | --- |
| `int2` `int4` | `number` |
| `int8` | `number`, or `bigint` when the value would not survive one |
| `float4` `float8` | `number` |
| `numeric` | `string` — arbitrary precision by definition, and a double is the one representation guaranteed to lose it |
| `bool` | `boolean` |
| `bytea` | `Uint8Array` |
| `json` `jsonb` | parsed |
| `timestamptz` `timestamp` | `Date` |
| everything else | `string` |

Parameters bind by **position** — `$1`, `$2`, or the `sql` tag. PostgreSQL's
wire protocol has no named parameters, and supporting `:name` would mean parsing
SQL in the driver, which is the one thing a driver should not do.

## Authentication

**SCRAM-SHA-256** (the default since PostgreSQL 14) and cleartext, both over
WebCrypto. `md5` is not implemented: it is deprecated upstream, and the runtime
has no MD5 to implement it with. The server proves itself to the client as well
as the reverse — the mutual half of SCRAM is verified, not skipped.

## What is not here yet

- **Binary result formats.** Everything is text for now. Binary is the larger
  win for numeric-heavy results and is next.
- **A connection pool.** One connection per `connect()`.
- **`COPY`**, `LISTEN`/`NOTIFY`, and cursors held across transactions.
- **Prepared-statement caching** — each query re-parses.

## Development

Tests need a PostgreSQL to talk to:

```sh
docker run -d --name esrun-pg-test \
  -e POSTGRES_PASSWORD=esrun -e POSTGRES_DB=esrun_test \
  -p 127.0.0.1:5433:5432 postgres:latest

bun run build
./test/run.sh                 # smoke, conformance, TLS negotiation
docker rm -f esrun-pg-test
```

`PG_URL` overrides the connection string.

## License

Apache-2.0
