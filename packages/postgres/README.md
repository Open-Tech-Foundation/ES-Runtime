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

## Running a script

`query()` and `execute()` use the extended protocol, which prepares the
statement — and a prepared statement is one statement by definition, so a string
with two of them is refused:

```js
await db.execute("SELECT 1; SELECT 2");   // ERR_DB_SYNTAX
```

That is the right answer for a query and the wrong one for a migration, so
scripts get their own door:

```js
import { connect } from "@opentf/esrun-postgres";

const db = await connect(url);
const results = await db.executeScript(`
  CREATE TABLE users (id serial PRIMARY KEY, name text NOT NULL);
  CREATE INDEX users_name ON users (name);
`);
// [{ command: "CREATE", changes: 0 }, { command: "CREATE", changes: 0 }]
```

Two things to know. **It takes no parameters** — the simple protocol has nowhere
to put them, so anything variable would have to be quoted into the text, and
quoting values into SQL is how injection happens. Use it for schema and fixed
statements, never for data from outside. And PostgreSQL wraps a multi-statement
string in a **single implicit transaction**, so a failure part-way rolls back
everything before it, unless the script manages its own transactions.

Rows are discarded: it reports what each statement did, not what it returned.

`executeScript` is on `PgConnection` rather than the portable `Connection`
surface — reach it through this package's own `connect()`.

## LISTEN / NOTIFY

```js
const listener = await connect(url);
listener.onNotification = ({ channel, payload }) => console.log(channel, payload);

await listener.listen("orders");
```

**A listening connection is dedicated.** A notification arrives when it arrives,
and a connection only sees messages while it is reading — which an idle one is
not. So the first `listen()` gives the connection over to a read loop, and from
then on it runs no queries: `query()` and `execute()` refuse with
`ERR_DB_CONNECTION_BUSY`. That is how you would deploy it anyway — a connection
that must notice a notification promptly should not be waiting behind someone's
report query. Use a second connection, or a pool, for the work.

`listen()` and `unlisten()` **await confirmation**, so a misspelled channel
fails there rather than silently never firing. The read loop owns reading; a
`LISTEN` only needs writing, and TCP is full duplex, so commands go out
underneath the loop and it settles them when their reply comes back.

Channel names are quoted as identifiers, so a name with a space or a quote in it
works and a name from somewhere else cannot become syntax. `payload` is `""`
when the notifier sent none, and `processId` identifies the sending backend —
which is how a connection recognises its own notifications, since PostgreSQL
delivers them to the sender too.

`onListenError` is called if the loop itself fails, since nobody is awaiting it.

## Notices and server parameters

```js
db.onNotice = (n) => console.warn(`${n.severity}: ${n.message}`);
```

Called for each `NOTICE`/`WARNING` the server sends — `RAISE NOTICE` in a
function, a deprecation, a truncation. A notice is the server talking, not the
statement failing, so it is never thrown. Unset, notices are **discarded rather
than printed**: a driver that wrote to stderr on its own would be one you had to
work around.

`db.parameters` tracks the settings the server reports, and keeps tracking them
— it is not a snapshot of the handshake:

```js
await db.execute("SET TIME ZONE 'Asia/Kolkata'");
db.parameters.TimeZone;   // "Asia/Kolkata"
```

## Pooling

One connection is one conversation, so concurrent work on a single connection is
not concurrent — it queues. A pool is how you get parallelism:

```js
import { createPool } from "@opentf/esrun-postgres";

const db = createPool(url, { max: 10 });

await Promise.all([db.query(a), db.query(b), db.query(c)]);   // actually parallel
await db.close();
```

It presents the same surface as a connection — `query`, `execute`,
`executeMany`, `transaction`, `executeScript` — and borrows one per operation.
Nothing is opened until something asks for work.

| Option | Default |
| --- | --- |
| `max` | 10 |
| `idleTimeout` (ms) | 30 000 |
| `acquireTimeout` (ms) | 10 000 |

It also works through `runtime:db` itself, so a pool is not something only this
package's own entry point can give you:

```js
const db = await connect("postgres://…", { pool: { max: 10 } });
```

**What is returned to the pool, and what is thrown away.** A connection goes
back only when PostgreSQL's own `ReadyForQuery` last said `I` — idle, outside
any transaction. `T` means a transaction is still open and `E` means one failed
and was never rolled back; either would leak into whoever borrowed it next, so
both are destroyed. A connection that died while nobody held it is checked for
on the way out too, because the first anyone hears of that is otherwise the next
caller's error.

`transaction()` holds one connection for the whole callback — a transaction
spread across connections is not a transaction. A streaming result holds its
connection until the rows run out; a result small enough to arrive in one batch
releases immediately, before the caller has read a row.

Idle connections are swept **on use, not on a timer**: a repeating timer would
keep the event loop alive for as long as the pool existed, so a program that had
finished its work would not exit. Call `close()` when you are done.

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

## Prepared statements

Every statement is prepared once per connection and reused, keyed by its SQL
text. `preparedStatementCacheSize` bounds it (default 100, `0` disables).

The bound matters as much as the cache. Each entry is a plan the **server**
holds, so an application generating unique SQL — a query builder inlining a
different literal each time — would otherwise accumulate them until the backend
ran out of memory. Eviction is least-recently-used and its `Close` rides along
with the next query rather than costing a round trip of its own.

A plan invalidated underneath you is handled rather than raised: if the server
says the cached plan is stale (`0A000`) or the statement is gone (`26000`,
after a pooler reset or `DISCARD ALL`), the cache is dropped and the statement
prepared again, once. Neither is the caller's mistake and neither surfaces as
one. `executeScript` clears the cache too, since DDL and `DISCARD` live there.

**On the speed of it:** measured against PostgreSQL in a container, caching is
worth about 6% per query — not the large win it is for an in-process engine.
A query here costs a network round trip, and parsing was never the dominant
term. It is still worth doing: it removes parse work from the server and the SQL
text from the wire on every repeat.

## Cancelling a query

```js
const controller = new AbortController();
setTimeout(() => controller.abort(), 5_000);

await db.query("SELECT * FROM slow", [], { signal: controller.signal });
```

Works on `query`, `execute` and `executeScript`, and `db.cancel()` cancels
whatever the connection is running without a signal.

The cancel goes out on a **second connection** — the protocol leaves no choice,
since the first is busy reading the answer to the very thing being cancelled.
That means cancellation is a *request*: the server may have finished already,
and the outcome shows up at the query rather than at `cancel()`.

Aborting sends the cancel and then **waits** for the server to answer, rather
than rejecting the caller immediately. The difference matters: rejecting at once
would leave a statement running and a connection mid-exchange, where waiting a
moment leaves both in a known state. The connection stays usable afterwards,
which is the whole difference between cancelling and hanging up.

What you get is your own `reason`, not the server's `57014` — including from a
result you abandon halfway, where the failure arrives out of the iterator rather
than out of the call that started it. A signal attached to a result that arrives
in one batch is detached immediately: cancelling a finished query means nothing.

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

## Environment

The `PG*` variables every libpq tool reads are honoured as **defaults**:
`PGHOST`, `PGPORT`, `PGUSER`, `PGPASSWORD`, `PGDATABASE`, `PGSSLMODE`,
`PGAPPNAME`, `PGCONNECT_TIMEOUT` (seconds, libpq's spelling).

```js
const db = await connect("postgres://");   // everything from the environment
```

Precedence, highest first: **explicit options → the URL → the environment →
the defaults**. `psql` behaves the same way, and a program that spelled out a
host should get that host whatever the shell exported. Only what the URL
actually carried counts as the URL having said anything, so `postgres://` names
no host and `PGHOST` still applies.

Reading the environment needs the **`Env`** capability. A program running
without it is not asking for libpq's defaults, so a refusal is not an error — it
means no defaults, and a connection string that named everything it needed still
works under `--deny-all`.

`PGSSLROOTCERT` is **not** read: libpq takes a path there, and reading a file is
a capability this driver will not exercise on your behalf. Pass the certificate
as `sslRootCert`.

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
| `timestamptz` | `Temporal.Instant` |
| `timestamp` | `Temporal.PlainDateTime` |
| `date` | `Temporal.PlainDate` |
| `time` | `Temporal.PlainTime` |
| `interval` | `Temporal.Duration` |
| arrays of the above | JS arrays, nested and null-aware |
| everything else | `string` |

**Dates and times are Temporal**, not `Date`. A `Date` is an instant with
millisecond resolution, which makes it the wrong type for three of these at
once: it cannot hold the microseconds a `timestamp` has, it can only express
`timestamp without time zone` by inventing a zone (and drivers disagree about
which — `postgres.js` uses the client's, so the same column reads differently on
different machines), and a `date` is a calendar day rather than an instant.

`connect(url, { temporal: false })` restores `Date` and strings for code that
has to hand a `Date` to something else.

A full table, including how Node, Bun and Deno's drivers map the same columns,
is at <https://es-runtime.opentechf.org/docs/db/postgres>.

An array of a type not in that list comes back as its raw literal
(`{"(1,2)"}`), because the wire does not say a column is an array — a column of
`int4[]` reports OID 1007 and nothing else — and guessing would corrupt a text
column that happens to contain braces.

Columns whose type is cheaper to read as bytes than as text — `int2` `int4`
`int8` `float4` `float8` `bool` `bytea` `uuid` `timestamp` `timestamptz` `date`
— are requested in the **binary** format. `numeric`, `json`/`jsonb` and arrays
stay text: binary `numeric` is a base-10000 digit array that is more work to
decode and no more exact, JSON has to be parsed as text at the end anyway, and
the array text parser already exists and is correct.

The wire format is how a value travels, not what it is: a column decodes to the
same JavaScript value either way, and a test asserts exactly that by reading the
same row through both paths.

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

The tests come in two halves. The **unit** tests need no database — a wire
codec is checkable on its own, and the cases worth pinning (a message split
across three chunks, a quoted `NULL` inside an array, RFC 7677's published SCRAM
vectors) are exactly the ones a live server will not produce on demand:

```sh
bun install
bun run build
./test/unit/run.sh
```

The **integration** tests need a real server, because speaking to one is the
whole point of the package:

```sh
docker run -d --name esrun-pg-test \
  -e POSTGRES_PASSWORD=esrun -e POSTGRES_DB=esrun_test \
  -p 127.0.0.1:5433:5432 postgres:latest

./test/run.sh                 # unit, then the suite against the server
docker rm -f esrun-pg-test
```

`PG_URL` overrides the connection string and `ESRUN` the binary. The TLS test
needs a server with a certificate from a private authority; `test/tls-server.sh`
stands one up and prints the environment for it.

Both halves run in CI, against a `postgres:18` service container.

## License

Apache-2.0
