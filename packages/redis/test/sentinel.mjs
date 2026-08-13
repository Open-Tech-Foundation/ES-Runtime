// Sentinel: finding the master, and finding it again after it moves.
//
// Needs a Sentinel deployment (see test/sentinel-server.sh). Skipped without
// one, because a test that quietly passes when it did not run is worse than no
// test.

import { connect } from "runtime:db";
import { env, exit } from "runtime:process";

import { driver as redis, redisSentinel, SentinelResolver } from "../dist/index.js";
import { is, ok, report } from "./unit/assert.mjs";

const sentinels = (env.REDIS_SENTINELS ?? "").split(",").filter(Boolean);
if (sentinels.length === 0) {
  console.log("skip sentinel — set REDIS_SENTINELS (see test/sentinel-server.sh)");
  exit(0);
}
const _container = env.REDIS_SENTINEL_CONTAINER ?? "esrun-redis-sentinel";
const masterName = "mymaster";

async function until(check, what, budget = 30000) {
  const deadline = Date.now() + budget;
  for (;;) {
    if (await check()) return true;
    if (Date.now() > deadline) {
      ok(false, `timed out waiting for ${what}`);
      return false;
    }
    await new Promise((r) => setTimeout(r, 200));
  }
}

/** Asks a sentinel directly, so the test's expectations do not come from the code under test. */
async function askSentinel() {
  const s = await connect(sentinels[0], { driver: redis });
  try {
    return await s.call(["SENTINEL", "get-master-addr-by-name", masterName]);
  } finally {
    await s.close();
  }
}

// -- resolving --------------------------------------------------------------

{
  const resolver = new SentinelResolver({ sentinels, masterName });
  const found = await resolver.resolve();
  const [host, port] = await askSentinel();
  is(`${found.host}:${found.port}`, `${host}:${port}`, "the resolver agrees with the sentinel");

  // The address it returns really is a master, which is the check that turns a
  // failover window from data loss into a retry.
  const direct = await connect(`redis://${found.host}:${found.port}`, { driver: redis });
  const role = await direct.call(["ROLE"]);
  is(role[0], "master", "and the address it returned is a master");
  await direct.close();
}

{
  // A sentinel that is down is the ordinary case rather than an exception —
  // that is what there are several of them for.
  const resolver = new SentinelResolver({
    sentinels: ["redis://127.0.0.1:1", ...sentinels],
    masterName,
    sentinelTimeout: 500,
  });
  const found = await resolver.resolve();
  ok(found.port > 0, "a dead sentinel at the front is skipped");
  // And the one that answered is moved to the front, so the next lookup does
  // not walk the dead one again.
  ok(
    resolver.sentinels[0] !== "redis://127.0.0.1:1",
    "the sentinel that answered is tried first next time",
  );
}

{
  // A master nobody has heard of is a configuration mistake, and it reads the
  // same from every sentinel — so it should say so rather than time out.
  const resolver = new SentinelResolver({ sentinels, masterName: "no-such-master" });
  let message = null;
  try {
    await resolver.resolve();
  } catch (e) {
    message = e.message;
  }
  ok(message?.includes("no-such-master"), "an unknown master name is named");
}

{
  let threw = false;
  try {
    new SentinelResolver({ sentinels: [], masterName });
  } catch {
    threw = true;
  }
  ok(threw, "a resolver with no sentinels is refused");
}

// -- a client -------------------------------------------------------------

{
  const r = await connect(sentinels[0], {
    driver: redisSentinel,
    sentinels: sentinels.slice(1),
    masterName,
  });
  is(await r.ping(), "PONG", "a sentinel client connects");
  await r.set("via-sentinel", "yes");
  is(await r.get("via-sentinel"), "yes", "and reads and writes the master");
  const role = await r.call(["ROLE"]);
  is(role[0], "master", "on a connection that really is to the master");
  await r.close();
}

{
  const pool = await connect(sentinels[0], {
    driver: redisSentinel,
    sentinels: sentinels.slice(1),
    masterName,
    pool: true,
  });
  is(await pool.ping(), "PONG", "a sentinel pool connects");
  await pool.set("pooled-sentinel", "yes");
  is(await pool.get("pooled-sentinel"), "yes", "and works");
  await pool.close();
}

// -- a failover -------------------------------------------------------------

{
  const before = await askSentinel();
  const r = await connect(sentinels[0], {
    driver: redisSentinel,
    sentinels: sentinels.slice(1),
    masterName,
    reconnect: true,
  });
  await r.set("survives-failover", "before");
  is(await r.get("survives-failover"), "before", "written to the original master");

  // Force one. Sentinel enforces a cooldown after a failover — a second one too
  // soon is refused with `-INPROG` — so this asks until it is accepted rather
  // than assuming the deployment is idle. That makes the test re-runnable,
  // which matters because the master alternates between runs.
  const admin = await connect(sentinels[0], { driver: redis });
  let accepted = false;
  const askDeadline = Date.now() + 40000;
  while (!accepted && Date.now() < askDeadline) {
    try {
      await admin.call(["SENTINEL", "FAILOVER", masterName]);
      accepted = true;
    } catch {
      await new Promise((r) => setTimeout(r, 1000));
    }
  }
  await admin.close();
  ok(accepted, "the sentinels accepted a failover request");

  await until(async () => {
    const now = await askSentinel();
    return now[1] !== before[1];
  }, "the sentinels to promote the replica");

  const after = await askSentinel();
  ok(after[1] !== before[1], `the master moved from ${before[1]} to ${after[1]}`);

  // The client should follow without the caller doing anything. The old server
  // is still answering, so this only works because a READONLY reply is treated
  // as "the master moved" rather than as an ordinary error.
  await until(async () => {
    try {
      await r.set("survives-failover", "after");
      return true;
    } catch {
      return false;
    }
  }, "the client to find the new master");

  is(await r.get("survives-failover"), "after", "the client writes to the new master");
  const role = await r.call(["ROLE"]);
  is(role[0], "master", "on a connection that is to a master again");
  is(r.hello.proto, 3, "with a full handshake, not a half-reused connection");
  await r.close();
}

{
  // A pool converges by doing what it does anyway: failed connections are
  // discarded, and every replacement resolves again.
  const pool = await connect(sentinels[0], {
    driver: redisSentinel,
    sentinels: sentinels.slice(1),
    masterName,
    pool: true,
  });
  await until(async () => {
    try {
      await pool.set("pool-after-failover", "ok");
      return true;
    } catch {
      return false;
    }
  }, "the pool to reach the current master");
  is(await pool.get("pool-after-failover"), "ok", "a pool works after a failover");
  await pool.close();
}

if (report("sentinel") > 0) exit(1);
