// The PG* variables are defaults below the URL and explicit options, as libpq
// has them. run.sh exports the ones for the test server, so each rung of the
// precedence is visible by what the server answers.
import { connect } from "runtime:db";
import { driver as postgres } from "../dist/index.js";

// `postgres://` names nothing, so everything comes from the environment.
const bare = await connect("postgres://", { driver: postgres });
console.log("connected from env:", (await (await bare.query("SELECT 5 AS n")).first()).n);
await bare.close();

// The URL wins over the environment: it names a database the server does not
// have, and the server says so, rather than PGDATABASE's being used.
try {
  await connect("postgres://127.0.0.1:5433/esrun_no_such_db", { driver: postgres });
  console.log("url wins: connected (should not happen)");
} catch (e) {
  console.log("url wins:", e.server?.code === "3D000", e.message.includes("esrun_no_such_db"));
}

// Explicit options win over both.
const explicit = await connect("postgres://127.0.0.1:5433/esrun_no_such_db", {
  driver: postgres,
  database: "esrun_test",
});
console.log("options win:", (await (await explicit.query("SELECT 6 AS n")).first()).n);
await explicit.close();
