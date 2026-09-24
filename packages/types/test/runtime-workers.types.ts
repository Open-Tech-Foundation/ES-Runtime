// A type test for `runtime:workers`' shard settings. Compiled by `tsc -p .`,
// never run.

import { configure, DurableErrorCode } from "runtime:workers";

const set = configure({
  shards: "auto",
  module: new URL("./classes.js", import.meta.url),
  permissions: ["net"],
});
const count: number = set.shards;
const module: string | null = set.module;
configure({ shards: 2, module: "file:///app/classes.js" });
configure({ shards: 0, module: null });

const lost: "ERR_DURABLE_SHARD_LOST" = DurableErrorCode.ShardLost;

// @ts-expect-error — a count, or "auto".
configure({ shards: "many" });

void count;
void module;
void lost;
