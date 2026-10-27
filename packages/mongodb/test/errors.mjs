// Server errors as DbErrors: a portable code, and the server's own on `server`.
import { connect, DbError } from "runtime:db";
import { env } from "runtime:process";
import { driver } from "../dist/index.js";
import { is, ok, report } from "./assert.mjs";

await using client = await connect(env.MONGODB_URL, { driver });

let e = null;
try {
  await client.command({ noSuchCommand: 1 });
} catch (error) {
  e = error;
}
ok(e instanceof DbError, "a failed command is a DbError");
is(
  [e?.code, e?.backendCode, e?.server?.codeName],
  ["ERR_DB_BACKEND", 59, "CommandNotFound"],
  "with the server's code and name",
);
ok(Array.isArray(e?.server?.errorLabels), "and its error labels");

const c = client.db().collection("errors");
await c.drop();
await c.insertOne({ _id: 1 });
e = null;
try {
  await c.insertOne({ _id: 1 });
} catch (error) {
  e = error;
}
is([e?.code, e?.backendCode], ["ERR_DB_UNIQUE_VIOLATION", 11000], "a duplicate key");
is(e?.server?.keyValue, { _id: 1 }, "names the key");

await client
  .db()
  .createCollection("errors_v", { validator: { $jsonSchema: { required: ["name"] } } });
e = null;
try {
  await client.db().collection("errors_v").insertOne({ other: 1 });
} catch (error) {
  e = error;
}
is(e?.code, "ERR_DB_CHECK_VIOLATION", "a document that fails validation");
await client.db().dropCollection("errors_v");

e = null;
try {
  await connect("mongodb://127.0.0.1:27017/?bogus=1", { driver });
} catch (error) {
  e = error;
}
is(e?.code, "ERR_DB_UNSUPPORTED", "an unknown option is refused, not ignored");
await c.drop();
if (report("errors") > 0) throw new Error("errors failed");
