import { connect } from "runtime:db";
import { env } from "runtime:process";
import { driver as postgres } from "../dist/index.js";

const url = env.PG_URL ?? "postgres://postgres:esrun@127.0.0.1:5433/esrun_test?sslmode=disable";
const db = await connect(url, { driver: postgres });
const one = async (sql, params) => await (await db.query(sql, params)).first();

// pg_prepared_statements is the server's own view of what this session holds,
// which is the only honest way to check a cache that lives on the other side.
const prepared = async () => (await one("SELECT count(*)::int AS n FROM pg_prepared_statements")).n;

// The count query is itself cached, so it is the baseline rather than zero.
const base = await prepared();
for (let i = 0; i < 5; i++) await one("SELECT $1::int AS v", [i]);
console.log("five runs of one statement prepare it once:", (await prepared()) - base === 1);

for (let i = 0; i < 3; i++) await one(`SELECT ${i}::int AS v`);
console.log("three distinct texts prepare three:", (await prepared()) - base === 4);

// The bound is the point: an application generating unique SQL would otherwise
// accumulate plans on the server until it ran out of memory.
const small = await connect(url, { driver: postgres, preparedStatementCacheSize: 2 });
for (let i = 0; i < 10; i++) {
  await (await small.query(`SELECT ${i}::int AS v`)).first();
}
const held = await (
  await small.query("SELECT count(*)::int AS n FROM pg_prepared_statements")
).first();
console.log("cache of 2 holds:", held.n <= 3, `(${held.n})`);
await small.close();

// Disabled means disabled.
const off = await connect(url, { driver: postgres, preparedStatementCacheSize: 0 });
for (let i = 0; i < 3; i++) await (await off.query("SELECT 1 AS v")).first();
const none = await (
  await off.query("SELECT count(*)::int AS n FROM pg_prepared_statements")
).first();
console.log("disabled holds:", none.n);
await off.close();

// A plan invalidated by a schema change must not surface as an application
// error: the table changed under it, which is nobody's mistake.
await db.execute("DROP TABLE IF EXISTS plans");
await db.execute("CREATE TABLE plans (a int)");
await db.execute("INSERT INTO plans VALUES (1)");
const q = "SELECT * FROM plans";
console.log("before change:", (await one(q)) !== null);
await db.executeScript("ALTER TABLE plans ADD COLUMN b int;");
const after = await one(q);
console.log("after change:", after !== null, "| columns:", Object.keys(after.toObject()).join(","));

// DISCARD ALL throws away every prepared statement the session had.
await db.executeScript("DISCARD ALL;");
console.log("after discard:", (await one("SELECT 1 AS v")).v);
await db.close();
