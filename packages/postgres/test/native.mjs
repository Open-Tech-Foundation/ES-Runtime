// The built-in driver (`postgres` from runtime:db, DECISIONS D147): the same
// server, the protocol in Rust. The conformance suite first, then what is this
// engine's own — the statement cache's bookkeeping across the op boundary, a
// result larger than one batch, and giving up on a handshake.
import { connect, DbErrorCode, postgres, runBackendConformance } from "runtime:db";
import { listen } from "runtime:net";
import { env, exit } from "runtime:process";
import { is, ok, report } from "./assert.mjs";

const url = env.PG_URL ?? "postgres://postgres:esrun@127.0.0.1:5433/esrun_test?sslmode=disable";

const conformance = await runBackendConformance(() => connect(url, { driver: postgres }));
for (const f of conformance.failures)
  console.log(`  FAIL conformance: ${f.name}\n       ${f.error}`);
ok(conformance.ok, "the conformance suite passes");

const db = await connect(url, { driver: postgres });
ok(db.parameters.server_version !== undefined, "the handshake's parameters are reported");

// Values both ways, in text and in binary.
const [row] = await (
  await db.query(
    "SELECT $1::int4 AS i, $2::int8 AS big, $3::text AS s, $4::bool AS b, $5::bytea AS raw, $6::jsonb AS doc, NULL::text AS nothing, $7::int4[] AS list",
    [41, 9007199254740993n, "héllo", true, new Uint8Array([1, 2, 255]), { a: [1] }, [1, null, 3]],
  )
).toArray();
is(row.i, 41, "int4");
is(String(row.big), "9007199254740993", "an int8 past 2^53 stays exact");
is(row.s, "héllo", "text");
is(row.b, true, "bool");
is([...row.raw], [1, 2, 255], "bytea");
is(row.doc, { a: [1] }, "jsonb");
is(row.nothing, null, "NULL");
is(row.list, [1, null, 3], "an int4[] with a NULL element");

// A server error keeps its SQLSTATE and everything else the server said.
try {
  await db.query("SELECT * FROM no_such_table_here");
  ok(false, "an undefined table is refused");
} catch (e) {
  is(e.code, DbErrorCode.UndefinedTable, "an undefined table maps to its portable code");
  is(e.server?.code, "42P01", "the SQLSTATE is kept");
}
is((await (await db.query("SELECT 1 AS n")).first()).n, 1, "the connection survives an error");

// A connection with a two-statement cache, run through three statements twice:
// every run past the second evicts one, and the row class that goes with it
// must go too, or a later statement reuses a stale shape.
const small = await connect(url, { driver: postgres, preparedStatementCacheSize: 2 });
for (let round = 0; round < 2; round++) {
  is((await (await small.query("SELECT 1 AS a")).first()).a, 1, `statement a, round ${round}`);
  is((await (await small.query("SELECT 'x' AS b")).first()).b, "x", `statement b, round ${round}`);
  is(
    (await (await small.query("SELECT true AS c")).first()).c,
    true,
    `statement c, round ${round}`,
  );
}
await small.close();

// Larger than one batch: the connection is held until the last row, and a
// second query meanwhile is refused by name rather than queued forever.
const many = await db.query("SELECT g FROM generate_series(1, 30000) g");
try {
  await db.query("SELECT 1");
  ok(false, "a second query while a result streams is refused");
} catch (e) {
  is(e.code, DbErrorCode.ConnectionBusy, "a second query while a result streams is refused");
}
let count = 0;
let last = 0;
for await (const r of many) {
  count++;
  last = r.g;
}
is([count, last], [30000, 30000], "every row of a multi-batch result arrives");

// Stopping early drains the rest, so the connection takes the next query.
for await (const _ of await db.query("SELECT g FROM generate_series(1, 30000) g")) break;
is(
  (await (await db.query("SELECT 2 AS n")).first()).n,
  2,
  "a connection is usable after an abandoned result",
);

// Transactions report their status, and a pool only reuses an idle connection.
await db.execute("CREATE TEMP TABLE native_t (x int)");
const changed = await db.transaction(async (tx) => {
  await tx.execute("INSERT INTO native_t VALUES (1), (2)");
  return tx.execute("UPDATE native_t SET x = x + 1");
});
is([changed.changes, db.status], [2, "I"], "a transaction commits and the connection is idle");
await db.close();

// Scripts: several statements, one string, each reporting its command.
const scripted = await connect(url, { driver: postgres });
const done = await scripted.executeScript(
  "CREATE TEMP TABLE native_s (x int); INSERT INTO native_s VALUES (1), (2); DELETE FROM native_s",
);
is(
  done.map((d) => [d.command, d.changes]),
  [
    ["CREATE", 0],
    ["INSERT", 2],
    ["DELETE", 2],
  ],
  "a script reports each statement",
);

// Cancelling from another task lands as the server's 57014 on the query.
const sleeping = scripted.query("SELECT pg_sleep(5)").then(
  () => "finished",
  (e) => e.server?.code,
);
await new Promise((resolve) => setTimeout(resolve, 200));
await scripted.cancel();
is(await sleeping, "57014", "cancel() stops a running statement");
is((await (await scripted.query("SELECT 6 AS n")).first()).n, 6, "and the connection survives it");
await scripted.close();

// LISTEN/NOTIFY: the subscribed connection delivers, and runs no queries.
const listener = await connect(url, { driver: postgres });
const notifier = await connect(url, { driver: postgres });
const heard = new Promise((resolve) =>
  listener.subscribe("native_channel", (payload, { channel }) => resolve([channel, payload])),
);
await new Promise((resolve) => setTimeout(resolve, 50));
await notifier.execute("SELECT pg_notify('native_channel', 'hello')");
is(await heard, ["native_channel", "hello"], "a NOTIFY reaches the subscriber");
is(listener.subscriptions, ["native_channel"], "the subscription is listed");
try {
  await listener.query("SELECT 1");
  ok(false, "a subscribed connection refuses queries");
} catch (e) {
  is(e.code, DbErrorCode.ConnectionBusy, "a subscribed connection refuses queries");
}
await listener.close();
await notifier.close();

// `postgres://` names nothing, so everything comes from the PG* variables
// run.sh exports — for one connection and for each a pool opens.
const bare = await connect("postgres://", { driver: postgres });
is((await (await bare.query("SELECT 3 AS n")).first()).n, 3, "PG* defaults reach a connection");
await bare.close();
const barePool = await connect("postgres://", { driver: postgres, pool: { max: 2 } });
is((await (await barePool.query("SELECT 4 AS n")).first()).n, 4, "PG* defaults reach a pool");
await barePool.close();

const pool = await connect(url, { driver: postgres, pool: { max: 4 } });
const sums = await Promise.all(
  Array.from(
    { length: 20 },
    async (_, i) => (await (await pool.query("SELECT $1::int * 2 AS n", [i])).first()).n,
  ),
);
is(
  sums.reduce((a, b) => a + b, 0),
  380,
  "a pool of four answers twenty queries",
);
await pool.close();

// A server that accepts and never answers: the connect timeout closes the
// socket, and nothing is left holding the event loop open.
const blackhole = listen({ hostname: "127.0.0.1", port: 0 });
const { port } = await blackhole.addr;
(async () => {
  // Held open deliberately: closing would give the client an EOF to react to.
  for await (const socket of blackhole) void socket;
})().catch(() => {});
const started = Date.now();
try {
  await connect(`postgres://postgres:esrun@127.0.0.1:${port}/x?sslmode=disable`, {
    driver: postgres,
    connectTimeout: 300,
  });
  ok(false, "a silent server times out");
} catch (e) {
  is(e.code, DbErrorCode.Timeout, "a silent server times out");
  ok(Date.now() - started < 2000, "and promptly");
}
await blackhole.close();

if (report("native") !== 0) exit(1);
