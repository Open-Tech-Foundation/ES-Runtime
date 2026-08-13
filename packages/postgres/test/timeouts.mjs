import { connect, DbErrorCode } from "runtime:db";
import { listen } from "runtime:net";
import { env } from "runtime:process";
import { driver as postgres } from "../dist/index.js";

const url = env.PG_URL ?? "postgres://postgres:esrun@127.0.0.1:5433/esrun_test?sslmode=disable";

// A server that completes the TCP handshake and then says nothing — the case
// that used to hang forever, and the reason connectTimeout exists. A refused
// connection would fail on its own; being *accepted* and ignored would not.
const blackhole = listen({ hostname: "127.0.0.1", port: 0 });
const { port } = await blackhole.addr;
(async () => {
  for await (const socket of blackhole) {
    // Held open deliberately: closing would give the client an EOF to react to.
    void socket;
  }
})().catch(() => {});

const started = performance.now();
try {
  await connect(`postgres://postgres:esrun@127.0.0.1:${port}/x?sslmode=disable`, {
    driver: postgres,
    connectTimeout: 400,
  });
  console.log("blackhole: connected (should not happen)");
} catch (e) {
  const elapsed = performance.now() - started;
  console.log("blackhole:", e.code === DbErrorCode.Timeout, "| under 2s:", elapsed < 2000);
}
await blackhole.close();

// statement_timeout is the server's to enforce: it cancels the statement and
// keeps the connection, which a client-side timer cannot do.
const db = await connect(url, { driver: postgres, statementTimeout: 300 });
console.log("guc:", db.parameters.statement_timeout ?? "(not reported)");
try {
  await db.execute("SELECT pg_sleep(3)");
  console.log("sleep: completed (should not happen)");
} catch (e) {
  console.log("sleep:", e.code === DbErrorCode.Timeout, "| sqlstate:", e.backendCode);
}
// The connection survives its own statement being cancelled.
console.log("still usable:", (await (await db.query("SELECT 1 AS n")).first()).n);
await db.close();

// The URL spells connect_timeout in seconds, libpq-style.
const viaUrl = await connect(`${url}&connect_timeout=5&statement_timeout=250`, {
  driver: postgres,
});
console.log("from url:", (await (await viaUrl.query("SELECT 2 AS n")).first()).n);
await viaUrl.close();
