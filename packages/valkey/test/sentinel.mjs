// A Valkey primary found through Sentinel, asked over valkey://.
//
// Needs a Sentinel deployment; see ../redis/test/sentinel-server.sh. Skipped
// without one.
import { connect } from "runtime:db";
import { env, exit } from "runtime:process";
import { ValkeyConnection, valkeySentinel } from "@opentf/esrun-valkey";
import { is, ok, report } from "./unit/assert.mjs";

const sentinels = (env.VALKEY_SENTINELS ?? "").split(",").filter(Boolean);
if (sentinels.length === 0) {
  console.log("skip sentinel — set VALKEY_SENTINELS (see test/run.sh)");
  exit(0);
}

const options = { driver: valkeySentinel, masterName: "mymaster", sentinels: sentinels.slice(1) };
{
  const v = await connect(sentinels[0], options);
  ok(v instanceof ValkeyConnection, "valkey:// sentinels resolve to a ValkeyConnection");
  is(v.backend, "valkey", "whose backend is valkey");
  is((await v.call(["ROLE"]))[0], "master", "pointed at the primary");
  await v.set("esrun:valkey:sentinel", "ok", { ex: 60 });
  is(await v.get("esrun:valkey:sentinel"), "ok", "which takes writes");
  await v.close();
}
{
  const pool = await connect(sentinels[0], { ...options, pool: true });
  is(await pool.ping(), "PONG", "a pool resolves through the sentinels too");
  await pool.close();
}

exit(report("sentinel") === 0 ? 0 : 1);
