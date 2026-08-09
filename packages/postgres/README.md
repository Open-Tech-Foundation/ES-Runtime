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
