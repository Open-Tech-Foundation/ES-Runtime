// A Valkey cluster, seeded with valkey:// URLs.
//
// Needs a cluster; see ../redis/test/cluster-server.sh. Skipped without one.
import { connect } from "runtime:db";
import { env, exit } from "runtime:process";
import { ValkeyCluster, valkeyCluster } from "@opentf/esrun-valkey";
import { is, ok, report } from "./unit/assert.mjs";

const seeds = (env.VALKEY_CLUSTER_URLS ?? "").split(",").filter(Boolean);
if (seeds.length === 0) {
  console.log("skip cluster — set VALKEY_CLUSTER_URLS (see test/run.sh)");
  exit(0);
}

const cluster = await connect(seeds[0], { driver: valkeyCluster, seeds: seeds.slice(1) });
ok(cluster instanceof ValkeyCluster, "valkey:// seeds open a ValkeyCluster");
is(cluster.backend, "valkey-cluster", "whose backend says so");
is(cluster.nodes.length, 3, "and which found the three primaries");

// Keys that hash to different slots, so more than one node is reached.
const keys = ["esrun:valkey:a", "esrun:valkey:b", "esrun:valkey:c", "esrun:valkey:d"];
for (const key of keys) await cluster.set(key, key, { ex: 60 });
const values = [];
for (const key of keys) values.push(await cluster.get(key));
is(values, keys, "commands are routed to the nodes that own their keys");
await cluster.close();

exit(report("cluster") === 0 ? 0 : 1);
