// Stopping a statement: a signal kills it on the server, and the connection lives on.
//
// The slow statement is a cross join rather than SLEEP(): MySQL answers a killed
// SLEEP() with 1, as if it had finished, where a killed query fails with
// ER_QUERY_INTERRUPTED — and only a failure is something a caller can see.
import { connect } from "runtime:db";
import { env } from "runtime:process";
import { driver } from "../dist/index.js";
import { is, ok, report } from "./unit/assert.mjs";

const url = env.MYSQL_URL ?? "mysql://root:esrun@127.0.0.1:3307/esrun_test?ssl-mode=DISABLED";
const db = await connect(url, { driver });
const SLOW =
  "SELECT COUNT(*) AS n FROM information_schema.columns a, information_schema.columns b, information_schema.columns c";

const started = Date.now();
let reason = null;
try {
  await db.query(SLOW, [], { signal: AbortSignal.timeout(200) });
} catch (e) {
  reason = e?.name ?? String(e);
}
const took = Date.now() - started;
ok(reason !== null, `the call rejected (${reason})`);
ok(took < 2000, `promptly (${took}ms)`);
is((await (await db.query("SELECT 1 AS one")).first()).one, 1, "the connection is still usable");

// cancel() with nothing running is harmless.
await db.cancel();
is((await (await db.query("SELECT 2 AS two")).first()).two, 2, "an idle cancel changes nothing");

// A server-side statement timeout: max_execution_time stops a SELECT.
const limited = await connect(url, { driver, statementTimeout: 100 });
let code = null;
try {
  await limited.query(SLOW);
} catch (e) {
  code = e.code;
}
is(code, "ERR_DB_TIMEOUT", "statementTimeout stops a long SELECT");
is(
  (await (await limited.query("SELECT 3 AS three")).first()).three,
  3,
  "and the connection carries on",
);
await limited.close();

await db.close();
if (report("cancel") > 0) throw new Error("cancel failed");
