// A real Valkey, reached as valkey://. The protocol under it is the Redis
// package's and is tested there — against Valkey too, in CI — so what is
// checked here is what this package adds: the scheme, the names, and that the
// server on the other end really is Valkey.
import { connect, DbErrorCode, queryAst } from "runtime:db";
import { env, exit, unmask } from "runtime:process";
import { driver, ValkeyConnection, ValkeyPooled } from "@opentf/esrun-valkey";
import { is, ok, report } from "./unit/assert.mjs";

const url = unmask(env.VALKEY_URL) ?? "valkey://127.0.0.1:6379";

{
  const v = await connect(url, { driver });
  ok(v instanceof ValkeyConnection, "valkey:// opens a ValkeyConnection");
  is(v.backend, "valkey", "whose backend is valkey");
  is(v.hello.server, "valkey", "and the server it reached says it is Valkey");
  is(v.protocol, 3, "over RESP3");

  await v.set("esrun:valkey:smoke", "ada", { ex: 60 });
  is(await v.get("esrun:valkey:smoke"), "ada", "a command round-trips");
  await v.execute(queryAst(["SET", "esrun:valkey:smoke", "grace"]));
  const rows = await v.query(queryAst(["GET", "esrun:valkey:smoke"]));
  let value = null;
  for await (const row of rows) value = row.value;
  is(value, "grace", "and so does the runtime:db half");

  // Valkey 9 added DELIFEQ; there is no helper for it, and `call` needs none.
  const [major] = String(v.hello.version).split(".");
  if (Number(major) >= 9) {
    is(
      Number(await v.call(["DELIFEQ", "esrun:valkey:smoke", "ada"])),
      0,
      "a Valkey-only command, refused",
    );
    is(Number(await v.call(["DELIFEQ", "esrun:valkey:smoke", "grace"])), 1, "and accepted");
  }

  let error = null;
  try {
    await v.query("SELECT 1");
  } catch (e) {
    error = e;
  }
  is(error?.code, DbErrorCode.QueryForm, "SQL text is refused");
  ok(String(error?.message).includes("valkey"), "by a message that says valkey");
  await v.close();
}

{
  // The same server under the Redis spelling, so a REDIS_URL keeps working.
  const v = await connect(url.replace(/^valkey(s?):/, "redis$1:"), { driver });
  is(v.backend, "valkey", "redis:// through this driver is still a Valkey connection");
  is(await v.ping(), "PONG", "and works");
  await v.close();
}

{
  const pool = await connect(url, { driver, pool: { max: 2 } });
  ok(pool instanceof ValkeyPooled, "pool: true gives a ValkeyPooled");
  const replies = await Promise.all([pool.ping(), pool.ping(), pool.ping()]);
  is(replies, ["PONG", "PONG", "PONG"], "which shares its connections");
  await pool.close();
}

exit(report("smoke") === 0 ? 0 : 1);
