# @opentf/esrun-mysql

A MySQL and MariaDB driver for [ES Runtime](https://esrun.opentechf.org)
(`esrun`), written **entirely in JavaScript** over `runtime:net`.

There is no native code in this package and none was added to the runtime for
it. That is the point of `runtime:db`: adding a database to this runtime does
not mean adding anything to the runtime.

```sh
npm install @opentf/esrun-mysql
```

```js
import { connect, sql } from "runtime:db";
import { driver } from "@opentf/esrun-mysql";   // the package's export *is* the driver

const db = await connect("mysql://user:secret@localhost/app", { driver });

await db.execute(sql`INSERT INTO users (name) VALUES (${name})`);

for await (const user of await db.query("SELECT id, name FROM users")) {
  console.log(user.id, user.name);
}

await db.close();
```

Everything in the [`runtime:db` guide](https://esrun.opentechf.org/docs/db)
works here — the `sql` tag, streaming results, transactions with savepoints,
`executeMany`, pooling, cancellation, and the portable error codes. This package
passes the same `runBackendConformance()` suite the built-in `sqlite:` backend
does, against MySQL 8.4 and MariaDB 11.

## Connection strings

```
mysql://user:password@host:3306/database?ssl-mode=REQUIRED
mariadb://user:password@host/database
```

| Part | Default |
| --- | --- |
| host | `localhost` (or `MYSQL_HOST`) |
| port | `3306` (or `MYSQL_TCP_PORT`) |
| user | `root` |
| password | none (or `MYSQL_PWD`) |
| `ssl-mode` | `PREFERRED` |

Options passed to `connect()` override the URL, and the URL overrides the
environment.

| Option | Meaning |
| --- | --- |
| `sslmode` | `"prefer"`, `"require"` or `"disable"` — the URL's `ssl-mode` |
| `sslRootCert` | a certificate authority to trust, as PEM |
| `connectTimeout` | ms to wait for the connection and its handshake (10 000) |
| `statementTimeout` | ms per statement, enforced by the server |
| `preparedStatementCacheSize` | statements kept prepared per connection (100) |
| `temporal` | decode dates and times to Temporal values (`true`) |

### TLS is verified

TLS here is verified, as it is everywhere in this runtime. A stock MySQL or
MariaDB server offers TLS with a certificate it generated and signed itself,
which nothing can verify — so against one, the default `ssl-mode=PREFERRED`
fails with an error that says so, rather than encrypting to a server it cannot
identify. Either give the driver the authority that signed the server's
certificate:

```js
import { file } from "runtime:fs";

const db = await connect("mysql://app@db.internal/app", {
  driver,
  sslRootCert: await file("/etc/ssl/internal-ca.pem").text(),   // a URL never reads files
});
```

or, on a network you trust, say so: `?ssl-mode=DISABLED`.

## Authentication

`caching_sha2_password` (MySQL's default) and `mysql_native_password` (MariaDB's)
are both spoken, including a server switching plugins mid-login. When
`caching_sha2_password` needs the password itself — the first login after the
server restarts — it is sent over TLS as it is, and over a plaintext connection
encrypted to the server's RSA key. `mysql_clear_password` is refused: this
driver never sends a password in the clear.

## Types

| MySQL | JavaScript |
| --- | --- |
| `TINYINT` … `INT`, `YEAR` | `number` |
| `BIGINT` | `number`, or `bigint` beyond ±2^53 |
| `FLOAT`, `DOUBLE` | `number` |
| `DECIMAL` | `string` — exact |
| `CHAR`, `VARCHAR`, `TEXT`, `ENUM`, `SET` | `string` |
| `BINARY`, `VARBINARY`, `BLOB`, `BIT`, `GEOMETRY` | `Uint8Array` |
| `JSON` | the parsed document |
| `DATE` | `Temporal.PlainDate` |
| `DATETIME` | `Temporal.PlainDateTime` |
| `TIMESTAMP` | `Temporal.Instant` |
| `TIME` | `Temporal.Duration` — `TIME` runs to ±838 hours |

The session is set to UTC at connect, which is what makes a `TIMESTAMP` the
instant it is rather than a wall time in the server's zone. The zero date
`0000-00-00` has no calendar value and comes back as the string MySQL prints.
With `temporal: false`, `DATETIME` and `TIMESTAMP` are `Date`s and `DATE` and
`TIME` are strings.

Parameters go the other way: `bigint` up to the unsigned 64-bit range,
`Uint8Array` as a blob, `Date` and every Temporal type as the matching MySQL
type, and plain objects and arrays as JSON text.

## Statements

`query()` and `execute()` prepare each statement once per connection and reuse
it, closing the least recently used beyond `preparedStatementCacheSize`. The few
statements MySQL will not prepare — `USE`, `XA`, `HELP` — run through the text
protocol instead, when they take no parameters.

Parameters bind by position, with `?` or through the `sql` tag. Named
parameters are refused rather than rewritten into the SQL.

A `CALL` returns its procedure's first result set; the rest are read and
discarded, so the connection is ready for the next statement.

## Running a script

```js
const results = await db.executeScript(`
  CREATE TABLE users (id INT AUTO_INCREMENT PRIMARY KEY, name VARCHAR(80) NOT NULL);
  CREATE INDEX users_name ON users (name);
`);
// [{ changes: 0, lastInsertRowid: null }, { changes: 0, lastInsertRowid: null }]
```

A script runs several statements in one string through the text protocol. **It
takes no parameters** — use it for schema and fixed statements, never for data
from outside. Unlike PostgreSQL, MySQL does **not** wrap a script in a
transaction, and DDL commits implicitly: a failure part-way leaves everything
before it done.

## Cancelling

```js
await db.query(report, [], { signal: AbortSignal.timeout(5_000) });
```

MySQL has no cancel message on the connection running the statement, so the
driver opens a second connection as the same user and runs `KILL QUERY`. The
statement fails, and the original connection stays open and usable. `cancel()`
does the same on demand.

`statementTimeout` is the server-side alternative: MySQL's `max_execution_time`,
which applies to `SELECT` only, or MariaDB's `max_statement_time`, which applies
to everything.

## Pooling

```js
const db = await connect(url, { driver, pool: { max: 10 } });
```

The same surface as a connection, borrowing one per operation. A connection is
returned only when the server's status says it is outside a transaction;
otherwise it is closed, so an open transaction never leaks into whoever borrows
it next.

## Testing

```sh
docker run -d -p 3307:3306 -e MYSQL_ROOT_PASSWORD=esrun -e MYSQL_DATABASE=esrun_test mysql:8.4
tsr build && packages/mysql/test/run.sh
eval "$(packages/mysql/test/tls-server.sh)"   # for the TLS test
```
