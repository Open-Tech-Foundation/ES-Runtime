// A real cluster: three primaries, 16384 slots, and the redirects between them.
//
// Needs a cluster (see test/cluster-server.sh). Skipped without one, because a
// test that quietly passes when it did not run is worse than no test.
import { exit, env } from "runtime:process";
import { connect, DbErrorCode } from "runtime:db";

import redis, { redisCluster, RedisCluster, parseConnectionString } from "../dist/index.js";
import { hashSlot } from "../dist/protocol/slots.js";
import { is, ok, report } from "./unit/assert.mjs";

const seeds = (env.REDIS_CLUSTER_URLS ?? "").split(",").filter(Boolean);
if (seeds.length === 0) {
  console.log("skip cluster — set REDIS_CLUSTER_URLS (see test/cluster-server.sh)");
  exit(0);
}

const cluster = await connect(seeds[0], { driver: redisCluster, seeds: seeds.slice(1) });

// -- the topology -----------------------------------------------------------

is(cluster.nodes.length, 3, `the cluster reported its three primaries`);
{
  // Every slot has an owner, which is the only useful definition of a healthy
  // cluster from a client's point of view.
  let unowned = 0;
  for (let slot = 0; slot < 16384; slot++) {
    if (cluster.nodeForSlot(slot) === undefined) unowned++;
  }
  is(unowned, 0, "every one of the 16384 slots has an owner");
}
{
  const owners = new Set();
  for (let slot = 0; slot < 16384; slot++) owners.add(cluster.nodeForSlot(slot));
  is(owners.size, 3, "the slots are spread across three primaries");
}

// -- keys land on the right node --------------------------------------------

{
  // Keys chosen to hash into different slots, so this genuinely exercises more
  // than one node rather than happening to stay on one.
  const keys = ["foo", "bar", "hello", "user:1", "user:2", "session:abc", "z", "counter"];
  const nodes = new Set(keys.map((k) => cluster.nodeForSlot(hashSlot(k))));
  ok(nodes.size > 1, `the test keys span ${nodes.size} nodes`);

  for (const key of keys) await cluster.set(key, `value-of-${key}`);
  for (const key of keys) {
    is(await cluster.get(key), `value-of-${key}`, `${key} (slot ${hashSlot(key)}) round-tripped`);
  }
}

// -- the client really did route, rather than getting lucky -----------------

{
  // Asking the owning node directly must find the key, and asking a different
  // node must not — which is what proves the routing rather than assuming it.
  const key = "routing-proof";
  await cluster.set(key, "here");
  const owner = cluster.nodeForSlot(hashSlot(key));
  const other = cluster.nodes.find((n) => n !== owner);

  const direct = await connect(`redis://${owner}`, { driver: redis });
  is(await direct.get(key), "here", "the owning node has the key");
  await direct.close();

  const wrong = await connect(`redis://${other}`, { driver: redis });
  let code = null;
  try {
    await wrong.get(key);
  } catch (e) {
    code = e.backendCode;
  }
  is(code, "MOVED", "and another node redirects rather than answering");
  await wrong.close();
}

// -- following a redirect ---------------------------------------------------

{
  // A client that has *not* read the topology. Every key it does not happen to
  // guess right gets a MOVED, so this exercises following them rather than
  // asserting that the map was correct all along.
  const blind = new RedisCluster([parseConnectionString(seeds[0])]);
  is(blind.nodes.length, 0, "a client that has not refreshed knows no topology");

  const keys = ["foo", "bar", "hello", "user:1", "user:2"];
  for (const key of keys) {
    is(await blind.get(key), `value-of-${key}`, `${key} answered after following redirects`);
  }

  // And it remembered: the slots it was redirected for now point somewhere.
  let learned = 0;
  for (const key of keys) {
    if (blind.nodeForSlot(hashSlot(key)) !== undefined) learned++;
  }
  is(learned, keys.length, "and every slot it was redirected for is now mapped");
  await blind.close();
}

{
  // A redirect loop must end rather than spin. With no redirects to follow the
  // bound is never reached, so this checks the bound is honoured by setting it
  // to something a blind client would exceed.
  const impatient = new RedisCluster([parseConnectionString(seeds[0])], { maxRedirects: 0 });
  let failed = false;
  try {
    // Deliberately a key the seed does not own, so a redirect is certain.
    const onOtherNode = ["foo", "bar", "hello"].find(
      (k) => cluster.nodeForSlot(hashSlot(k)) !== `127.0.0.1:${new URL(seeds[0]).port}`,
    );
    await impatient.get(onOtherNode);
  } catch (e) {
    failed = e.code === DbErrorCode.Unsupported;
  }
  ok(failed, "a redirect budget of zero gives up rather than following one");
  await impatient.close();
}

// -- CROSSSLOT --------------------------------------------------------------

{
  // Two keys in different slots have no node that owns both. The server would
  // say CROSSSLOT; a multi-key command should surface that clearly.
  ok(hashSlot("foo") !== hashSlot("bar"), "foo and bar are in different slots");
  let code = null;
  try {
    await cluster.mget("foo", "bar");
  } catch (e) {
    code = e.backendCode ?? e.code;
  }
  ok(code === "CROSSSLOT" || code === DbErrorCode.Unsupported,
    `a cross-slot MGET is refused (${code})`);
}

{
  // Hash tags are the answer, and they work: keys sharing a tag share a slot,
  // so a multi-key command over them is legal.
  is(hashSlot("{cart:9}:items"), hashSlot("{cart:9}:total"), "tagged keys share a slot");
  await cluster.set("{cart:9}:items", "3");
  await cluster.set("{cart:9}:total", "42");
  is(await cluster.mget("{cart:9}:items", "{cart:9}:total"), ["3", "42"],
    "so MGET across them works");
}

// -- transactions and pipelines ---------------------------------------------

{
  // A transaction has to run on one node, so its keys must share a slot.
  // Re-runnable: a cluster keeps its data between runs, and a counter that
  // remembered the last one would fail on the second.
  await cluster.del("{order:1}:version");
  const tx = cluster.multi();
  tx.set("{order:1}:state", "paid");
  tx.incr("{order:1}:version");
  is(await tx.exec(), ["OK", 1], "a single-slot transaction runs");

  const spanning = cluster.multi();
  spanning.set("foo", "1");
  spanning.set("bar", "2");
  let code = null;
  try {
    await spanning.exec();
  } catch (e) {
    code = e.backendCode;
  }
  is(code, "CROSSSLOT", "and one spanning two slots is refused by name");
}

{
  // A pipeline may span nodes: it is split per node, each group is still one
  // round trip, and the groups run at the same time.
  const p = cluster.pipeline();
  const keys = ["foo", "bar", "hello", "user:1", "user:2"];
  for (const key of keys) p.get(key);
  const results = await p.exec();
  is(results.length, keys.length, "every command in a cross-node pipeline answered");
  is(results, keys.map((k) => `value-of-${k}`), "each with its own key's value, in order");
}

// -- EVAL is routed by its key, not its script ------------------------------

{
  // Routing EVAL by argument 1 would hash the script text and scatter scripts
  // across nodes at random. The key after numkeys is the one that matters.
  const value = await cluster.eval("return redis.call('GET', KEYS[1])", ["user:1"], []);
  is(value, "value-of-user:1", "EVAL reaches the node that owns its key");
}

// -- keyless commands -------------------------------------------------------

is(await cluster.ping(), "PONG", "a keyless command goes anywhere and works");

// -- closing ----------------------------------------------------------------

await cluster.close();
{
  let code = null;
  try {
    await cluster.get("foo");
  } catch (e) {
    code = e.code;
  }
  is(code, DbErrorCode.Closed, "a closed cluster client refuses work");
}

if (report("cluster") > 0) exit(1);
