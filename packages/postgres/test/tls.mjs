import { driver as postgres } from "../dist/index.js";
import { env } from "runtime:process";
import { connect } from "runtime:db";
// No sslmode: the default is "prefer" — ask for TLS, continue without it if the
// server says no. This server has ssl off, so it exercises the fallback.
const db = await connect((env.PG_URL ?? "postgres://postgres:esrun@127.0.0.1:5433/esrun_test").replace(/[?].*$/, ""), {
  driver: postgres,
});
console.log("prefer:", (await (await db.query("SELECT 1 AS n")).first()).n);
await db.close();
try {
  await connect(
    (env.PG_URL ?? "postgres://postgres:esrun@127.0.0.1:5433/esrun_test").replace(/[?].*$/, "") +
      "?sslmode=require",
    { driver: postgres },
  );
  console.log("require: connected (server has TLS)");
} catch (e) {
  console.log("require:", e.code, "|", /refused TLS/.test(e.message));
}
