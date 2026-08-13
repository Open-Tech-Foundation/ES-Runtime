import { connect } from "runtime:db";
import { env } from "runtime:process";
import { driver as postgres } from "../dist/index.js";

const url = env.PG_URL ?? "postgres://postgres:esrun@127.0.0.1:5433/esrun_test?sslmode=disable";
const pool = await connect(url, { driver: postgres, pool: { max: 3 } });

// Nothing is opened until something asks for work.
console.log("starts empty:", pool.size === 0);
console.log("query:", (await (await pool.query("SELECT 1 AS n")).first()).n);
console.log("one connection:", pool.size === 1, "| idle after:", pool.idle === 1);

// A small result frees the connection before the caller reads a row, so the
// second query reuses the first connection rather than opening another.
await (await pool.query("SELECT 2 AS n")).first();
console.log("reused:", pool.size === 1);

// Real concurrency: three queries at once need three connections, where a
// single connection would have serialised them.
const started = performance.now();
await Promise.all([
  pool.execute("SELECT pg_sleep(0.2)"),
  pool.execute("SELECT pg_sleep(0.2)"),
  pool.execute("SELECT pg_sleep(0.2)"),
]);
const elapsed = performance.now() - started;
console.log(
  "concurrent:",
  pool.size === 3,
  "| overlapped:",
  elapsed < 450,
  `(${elapsed.toFixed(0)}ms)`,
);

// A streaming result holds its connection until the rows run out.
const rows = await pool.query("SELECT g FROM generate_series(1, 20000) g");
console.log("streaming holds one:", pool.idle === 2);
let seen = 0;
for await (const _r of rows) seen++;
console.log("drained:", seen === 20000, "| returned:", pool.idle === 3);

// A failed transaction leaves the session in a state nobody else should
// inherit, so that connection is destroyed rather than pooled.
const before = pool.size;
try {
  await pool.transaction(async (tx) => {
    await tx.execute("SELECT 1");
    throw new Error("rollback");
  });
} catch {}
console.log("after rollback, still pooled:", pool.size === before);

// A connection killed underneath the pool is not handed to the next caller.
const victim = await (await pool.query("SELECT pg_backend_pid() AS pid")).first();
await pool.execute("SELECT pg_terminate_backend($1)", [victim.pid]).catch(() => {});
console.log("survives a killed backend:", (await (await pool.query("SELECT 3 AS n")).first()).n);

await pool.close();
console.log("closed:", pool.idle === 0);

// A second pool over the same URL is a second pool, and closing the first left
// nothing behind that would stop it.
const another = await connect(url, { driver: postgres, pool: { max: 2 } });
console.log("a second pool:", (await (await another.query("SELECT 4 AS n")).first()).n);
await another.close();
