import { env } from "runtime:process";
import postgres from "../dist/index.js";
import { connect } from "runtime:db";

const url = env.PG_URL ?? "postgres://postgres:esrun@127.0.0.1:5433/esrun_test?sslmode=disable";
const db = await connect(url, { driver: postgres });

// An abort reaches a query already running. The cancel goes on its own
// connection, because this one is busy reading the answer to the very thing
// being cancelled.
const controller = new AbortController();
setTimeout(() => controller.abort(new Error("changed my mind")), 300);
const started = performance.now();
let seen = "completed";
try {
  await db.execute("SELECT pg_sleep(10)", [], { signal: controller.signal });
} catch (e) {
  seen = e.message;
}
const elapsed = performance.now() - started;
console.log("aborted:", seen === "changed my mind", "| promptly:", elapsed < 3000, `(${elapsed.toFixed(0)}ms)`);

// The connection survives its own statement being cancelled — that is the whole
// difference between cancelling and hanging up.
console.log("still usable:", (await (await db.query("SELECT 1 AS n")).first()).n);

// A signal already aborted never reaches the server.
const already = AbortSignal.abort(new Error("too late"));
let early = "ran anyway";
try {
  await db.execute("SELECT 1", [], { signal: already });
} catch (e) {
  early = e.message;
}
console.log("pre-aborted:", early === "too late");

// A signal that never fires costs nothing and changes nothing.
const quiet = new AbortController();
console.log("unaborted:", (await (await db.query("SELECT 2 AS n", [], { signal: quiet.signal })).first()).n);

// Cancelling a streaming result mid-iteration: the signal stays attached until
// the rows end, because a streaming result is still the query running.
//
// The rows have to be wide enough and slow enough to genuinely stream. A result
// that fits one batch is already complete when it arrives — `exhausted` detaches
// the signal precisely because cancelling a finished query means nothing.
const stream = new AbortController();
let rows = 0;
let streamError = "none";
const streamStart = performance.now();
try {
  const result = await db.query(
    "SELECT g, repeat('x', 2000) AS pad, pg_sleep(0.002) FROM generate_series(1, 20000) g",
    [],
    { signal: stream.signal },
  );
  console.log("streaming (not one batch):", result.exhausted === false);
  for await (const _row of result) {
    if (++rows === 2) stream.abort(new Error("enough"));
  }
} catch (e) {
  streamError = e.message;
}
const streamElapsed = performance.now() - streamStart;
console.log(
  "stream aborted:",
  streamError === "enough",
  `| stopped early: ${rows < 20000}`,
  `| promptly: ${streamElapsed < 20000}`,
);

// And after all that the connection is still the one we started with.
console.log("survives:", (await (await db.query("SELECT 3 AS n")).first()).n);

// cancel() with nothing running is a no-op rather than an error.
await db.cancel();
console.log("idle cancel is harmless:", (await (await db.query("SELECT 4 AS n")).first()).n);
await db.close();
