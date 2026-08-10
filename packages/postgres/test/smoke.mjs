import postgres from "../dist/index.js";
import { env } from "runtime:process";
import { connect, sql, DbErrorCode } from "runtime:db";

const url = env.PG_URL ?? "postgres://postgres:esrun@127.0.0.1:5433/esrun_test?sslmode=disable";
const db = await connect(url, { driver: postgres });
console.log("connected, server:", db.parameters.server_version);

await db.execute("DROP TABLE IF EXISTS smoke");
await db.execute("CREATE TABLE smoke (id serial PRIMARY KEY, name text NOT NULL, score float8, big int8, data bytea, meta jsonb, at timestamptz)");
const r = await db.execute(
  "INSERT INTO smoke (name, score, big, data, meta, at) VALUES ($1, $2, $3, $4, $5, $6)",
  ["ada", 9.5, 9007199254740993n, new Uint8Array([1, 2, 3]), { a: 1 }, new Date("2026-01-02T03:04:05Z")],
);
console.log("insert changes:", r.changes);

await db.execute(sql`INSERT INTO smoke (name, score) VALUES (${"grace"}, ${8})`);

const rows = await (await db.query("SELECT id, name, score, big, data, meta, at FROM smoke ORDER BY id")).toArray();
console.log("rows:", rows.length);
const first = rows[0];
console.log("types:", typeof first.id, typeof first.name, typeof first.score, typeof first.big, first.data?.constructor?.name, typeof first.meta, first.at?.constructor?.name);
console.log("bigint:", first.big.toString());
console.log("bytea:", [...first.data].join("-"));
console.log("json:", JSON.stringify(first.meta));
console.log("null score:", rows[1].big);

await db.transaction(async (tx) => { await tx.execute("INSERT INTO smoke (name) VALUES ('in-tx')"); });
try {
  await db.transaction(async (tx) => {
    await tx.execute("INSERT INTO smoke (name) VALUES ('rolled-back')");
    throw new Error("no");
  });
} catch {}
console.log("count:", (await (await db.query("SELECT count(*)::int AS n FROM smoke")).first()).n);

try {
  await db.execute("INSERT INTO smoke (id, name) VALUES (1, 'clash')");
} catch (e) {
  console.log("unique:", e.code === DbErrorCode.UniqueViolation, "| sqlstate:", e.backendCode);
}
try {
  await db.query("SELECT * FROM nope");
} catch (e) {
  console.log("undefined table:", e.code === DbErrorCode.UndefinedTable, "| sqlstate:", e.backendCode);
}
await db.close();
console.log("closed");
