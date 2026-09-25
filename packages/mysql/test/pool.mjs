// A pool: the same surface, connections reused when clean and dropped when not.
import { connect } from "runtime:db";
import { env } from "runtime:process";
import { driver } from "../dist/index.js";
import { is, ok, report } from "./unit/assert.mjs";

const url = env.MYSQL_URL ?? "mysql://root:esrun@127.0.0.1:3307/esrun_test?ssl-mode=DISABLED";
const pool = await connect(url, { driver, pool: { max: 4 } });

const ids = await Promise.all(
  Array.from(
    { length: 20 },
    async () => (await (await pool.query("SELECT CONNECTION_ID() AS id")).first()).id,
  ),
);
ok(new Set(ids).size <= 4, `20 queries share at most 4 connections (${new Set(ids).size})`);

await pool.execute("DROP TABLE IF EXISTS pool_t");
await pool.execute("CREATE TABLE pool_t (v INT)");
await pool.transaction(async (tx) => {
  await tx.execute("INSERT INTO pool_t VALUES (1)");
  await tx.transaction(async (inner) => {
    await inner.execute("INSERT INTO pool_t VALUES (2)");
  });
  try {
    await tx.transaction(async (inner) => {
      await inner.execute("INSERT INTO pool_t VALUES (3)");
      throw new Error("roll this savepoint back");
    });
  } catch {
    /* expected */
  }
});
is(
  (await (await pool.query("SELECT v FROM pool_t ORDER BY v")).toArray()).map((r) => r.v),
  [1, 2],
  "savepoints nest and roll back alone",
);

// A connection handed back inside a transaction is not handed to anyone else.
const leaked = await pool.withConnection(async (c) => {
  await c.execute("BEGIN");
  await c.execute("INSERT INTO pool_t VALUES (99)");
  return c.connectionId;
});
const after = await Promise.all(
  Array.from(
    { length: 8 },
    async () => (await (await pool.query("SELECT CONNECTION_ID() AS id")).first()).id,
  ),
);
ok(!after.includes(leaked), "a connection left in a transaction is discarded");
is(
  (await (await pool.query("SELECT COUNT(*) AS n FROM pool_t WHERE v = 99")).first()).n,
  0,
  "and its work never committed",
);

const script = await pool.executeScript("SELECT 1; SELECT 2");
is(script.length, 2, "scripts run on a borrowed connection");

await pool.execute("DROP TABLE pool_t");
await pool.close();
if (report("pool") > 0) throw new Error("pool failed");
