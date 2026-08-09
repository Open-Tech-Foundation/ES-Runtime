import "../dist/index.js";
import { env } from "runtime:process";
import { connect, DbErrorCode } from "runtime:db";
import { connect as pgConnect } from "../dist/index.js";

const url = env.PG_URL ?? "postgres://postgres:esrun@127.0.0.1:5433/esrun_test?sslmode=disable";
const db = await pgConnect(url);

// The extended protocol cannot carry two statements, and says so.
try {
  await db.execute("SELECT 1; SELECT 2");
  console.log("extended multi: accepted (should not happen)");
} catch (e) {
  console.log("extended multi refused:", e.code === DbErrorCode.Syntax);
}

const results = await db.executeScript(`
  DROP TABLE IF EXISTS script_a;
  DROP TABLE IF EXISTS script_b;
  CREATE TABLE script_a (i int);
  CREATE TABLE script_b (i int);
  INSERT INTO script_a VALUES (1), (2), (3);
`);
console.log("statements:", results.length);
console.log("commands:", results.map((r) => r.command).join(","));
console.log("insert changes:", results.find((r) => r.command === "INSERT")?.changes);

// PostgreSQL wraps a multi-statement string in one implicit transaction, so a
// failure part-way undoes what came before it.
try {
  await db.executeScript("INSERT INTO script_a VALUES (4); INSERT INTO script_a VALUES ('not an int');");
} catch (e) {
  console.log("mid-script failure:", e.code !== undefined);
}
const rows = await (await db.query("SELECT count(*)::int AS n FROM script_a")).first();
console.log("rolled back to:", rows.n);

// The connection is usable afterwards.
console.log("still usable:", (await (await db.query("SELECT 1 AS n")).first()).n);
await db.close();
