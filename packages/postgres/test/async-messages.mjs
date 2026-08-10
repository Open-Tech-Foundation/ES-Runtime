import { env } from "runtime:process";
import { driver as postgres } from "../dist/index.js";
import { connect } from "runtime:db";

const url = env.PG_URL ?? "postgres://postgres:esrun@127.0.0.1:5433/esrun_test?sslmode=disable";
const db = await connect(url, { driver: postgres });

// ParameterStatus arrives unprompted whenever the server thinks a tracked GUC
// changed, so `parameters` has to keep listening after the handshake.
const before = db.parameters.TimeZone;
await db.execute("SET TIME ZONE 'Asia/Kolkata'");
const after = db.parameters.TimeZone;
console.log("timezone tracked:", before !== after, "|", after);

// A notice is the server talking, not the statement failing.
const notices = [];
db.onNotice = (n) => notices.push(`${n.severity}:${n.message}`);
await db.executeScript("DO $$ BEGIN RAISE NOTICE 'from a script'; END $$;");
await db.execute("DO $$ BEGIN RAISE NOTICE 'from a statement'; END $$");
await db.query("DO $$ BEGIN RAISE WARNING 'from a query'; END $$");
console.log("notices:", notices.length);
console.log("severities:", notices.map((n) => n.split(":")[0]).join(","));
console.log("messages:", notices.map((n) => n.split(":")[1]).join("|"));

// Unhandled notices are dropped, not printed: a driver that wrote to stderr on
// its own would be one you had to work around.
db.onNotice = undefined;
await db.execute("DO $$ BEGIN RAISE NOTICE 'silent'; END $$");
console.log("after unset:", notices.length);
await db.close();
