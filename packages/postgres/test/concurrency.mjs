import { connect, DbErrorCode } from "runtime:db";
import { env } from "runtime:process";
import { driver as postgres } from "../dist/index.js";

const url = env.PG_URL ?? "postgres://postgres:esrun@127.0.0.1:5433/esrun_test?sslmode=disable";
const db = await connect(url, { driver: postgres });
await db.execute("DROP TABLE IF EXISTS conc");
await db.execute("CREATE TABLE conc (i int, pad text)");
await db.execute("INSERT INTO conc SELECT g, repeat('x', 200) FROM generate_series(1, 5000) g");

// A query issued while another result is still streaming used to deadlock: two
// readers taking turns on one socket, each waiting for the other's message. It
// is refused now, by name, and the refusal says what to do instead.
let refused = null;
let seen = 0;
for await (const _row of await db.query("SELECT i, pad FROM conc ORDER BY i")) {
  if (seen === 0) {
    try {
      await db.query("SELECT 42 AS answer");
    } catch (e) {
      refused = e.code;
    }
  }
  seen++;
}
console.log("refused:", refused === DbErrorCode.ConnectionBusy);
console.log("outer finished:", seen);

// The connection is usable again the moment the result set ends.
console.log("after drain:", (await (await db.query("SELECT 42 AS answer")).first()).answer);

// Breaking out early drains the rest and releases too.
let partial = 0;
for await (const _row of await db.query("SELECT i FROM conc")) {
  if (++partial === 3) break;
}
console.log("after break:", (await (await db.query("SELECT 1 AS n")).first()).n);

// A result that fits one batch never holds the connection at all.
const small = await db.query("SELECT 1 AS n");
console.log("small is free:", (await (await db.query("SELECT 2 AS n")).first()).n);
await small.toArray();

// Concurrent statements with no open result set queue rather than being
// refused: an exchange in flight finishes on its own, so waiting is finite.
const results = await Promise.all([
  db.execute("SELECT pg_sleep(0.05)"),
  db.execute("SELECT 1"),
  db.execute("SELECT 2"),
]);
console.log("queued:", results.length);

// A failed exchange releases the lock rather than poisoning the connection.
try {
  await db.query("SELECT * FROM nope");
} catch {}
console.log("after error:", (await (await db.query("SELECT 3 AS n")).first()).n);
await db.close();
