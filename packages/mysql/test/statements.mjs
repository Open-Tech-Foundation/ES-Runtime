// Prepared statements: cached, evicted, turned off, and the ones MySQL will not prepare.
import { connect } from "runtime:db";
import { env } from "runtime:process";
import { driver } from "../dist/index.js";
import { is, ok, report, throws } from "./unit/assert.mjs";

const url = env.MYSQL_URL ?? "mysql://root:esrun@127.0.0.1:3307/esrun_test?ssl-mode=DISABLED&allowPublicKeyRetrieval=true";
/** How many statements the server holds for `db`, asked from another connection. */
async function heldBy(watcher, db) {
  const row = await (
    await watcher.query(
      `SELECT COUNT(*) AS n FROM performance_schema.prepared_statements_instances p
       JOIN performance_schema.threads t ON p.OWNER_THREAD_ID = t.THREAD_ID
       WHERE t.PROCESSLIST_ID = ?`,
      [db.connectionId],
    )
  ).first();
  return Number(row.n);
}

// Counting what the server holds needs performance_schema, which MariaDB ships
// switched off.
const probe = await connect(url, { driver });
const instrumented =
  Number((await (await probe.query("SELECT @@performance_schema AS on_")).first()).on_) === 1;
await probe.close();
if (!instrumented) console.log("  skip statement counts: performance_schema is off");

// A small cache: the oldest statement is closed on the server when it is
// evicted — sent ahead of the next command, since a close gets no answer.
if (instrumented) {
  const db = await connect(url, { driver, preparedStatementCacheSize: 2 });
  const watcher = await connect(url, { driver });
  for (let i = 0; i < 5; i++) await db.query(`SELECT ${i} AS n`);
  await db.query("SELECT 'flush' AS n");
  is(await heldBy(watcher, db), 2, "the server holds what the cache holds");
  await db.close();
  await watcher.close();
}

// Caching off: each statement is closed once it has run.
if (instrumented) {
  const db = await connect(url, { driver, preparedStatementCacheSize: 0 });
  const watcher = await connect(url, { driver });
  for (let i = 0; i < 5; i++) await db.query(`SELECT ${i} AS n`);
  ok((await heldBy(watcher, db)) <= 1, "at most the last one, whose close rides the next command");
  await db.close();
  await watcher.close();
}

// The same text reused across different parameter counts is refused before the server.
{
  const db = await connect(url, { driver });
  await throws(() => db.query("SELECT ? AS a, ? AS b", [1]), "too few parameters is refused");
  const r = await (await db.query("SELECT ? AS a, ? AS b", [1, 2])).first();
  is([r.a, r.b], [1, 2], "the connection is fine afterwards");
  await db.close();
}

// A statement the prepared protocol refuses (ER_UNSUPPORTED_PS) runs as text:
// `USE` for an execute, `XA RECOVER` for a query that returns rows.
{
  const db = await connect(url, { driver });
  await db.execute("USE esrun_test");
  const recovered = await (await db.query("XA RECOVER")).toArray();
  ok(Array.isArray(recovered), "XA RECOVER answers through the text protocol");
  const help = await (await db.query("HELP 'contents'")).toArray();
  ok(help.length > 0 && typeof help[0].name === "string", "a text result decodes into rows");
  await throws(
    () => db.query("USE esrun_test_missing_db_xyz"),
    "a refused statement's own error still surfaces",
  );
  is(
    (await (await db.query("SELECT 2 AS two")).first()).two,
    2,
    "prepared statements still work after it",
  );
  await db.close();
}

// Schema changes under a cached statement: the server re-prepares.
{
  const db = await connect(url, { driver });
  await db.execute("DROP TABLE IF EXISTS reprep");
  await db.execute("CREATE TABLE reprep (a INT)");
  await db.execute("INSERT INTO reprep VALUES (1)");
  is(
    Object.keys((await (await db.query("SELECT * FROM reprep")).first()).toObject()),
    ["a"],
    "one column",
  );
  await db.execute("ALTER TABLE reprep ADD COLUMN b VARCHAR(4) DEFAULT 'x'");
  is(
    (await (await db.query("SELECT * FROM reprep")).first()).toObject(),
    { a: 1, b: "x" },
    "the new column appears",
  );
  await db.execute("DROP TABLE reprep");
  await db.close();
}

if (report("statements") > 0) throw new Error("statements failed");
