/**
 * `@opentf/esrun-redis` — the Redis driver for `runtime:db`.
 *
 * The package's exports *are* drivers. Nothing is registered by importing it,
 * because nothing needs to be: you hand a driver to `connect`, and what comes
 * back speaks both Redis's vocabulary and `runtime:db`'s.
 *
 * ```js
 * import { connect, queryAst } from "runtime:db";
 * import { driver } from "@opentf/esrun-redis";
 *
 * const r = await connect("redis://localhost", { driver });
 * await r.set("greeting", "hello", { ex: 60 });
 * await r.get("greeting");
 *
 * // The same connection, as a runtime:db backend.
 * const rows = await r.query(queryAst(["LRANGE", "log", 0, -1]));
 * ```
 *
 * Three drivers, one call. {@link driver} opens a connection — `pool: true`
 * makes it a pool with the same surface. {@link redisCluster} opens a cluster
 * client, and {@link redisSentinel} finds a master through Sentinel. Which
 * client you get follows from the driver you passed, rather than from which of
 * seven functions you happened to call.
 *
 * Every driver package exports its driver under the name `driver`, and nothing
 * as a default — so the import is the same shape whichever backend it is, and
 * `{ driver }` is the whole of the option. Two drivers in one module are
 * `import { driver as redis }`.
 *
 * There is no native code here, and none was added to the runtime for it. The
 * driver is JavaScript over `runtime:net` — the arrangement D56 committed to,
 * in the sentence that names Redis by name.
 */

import { RedisBatch, RedisPipeline, RedisTransaction } from "./batch.js";
import { RedisCluster, type RedisClusterOptions, redisCluster } from "./cluster.js";
import { mixinCommands, RedisCommands } from "./commands.js";
import {
  type MessageContext,
  type MessageHandler,
  REDIS_DIALECT,
  type ReconnectOptions,
  RedisConnection,
  type RedisOptions,
  type RedisPayload,
  type ServerHello,
} from "./connection.js";
import { driver, openConnection } from "./driver.js";
import { RedisPooled, type RedisPoolOptions } from "./pool.js";
import {
  redisSentinel,
  type SentinelDriverOptions,
  type SentinelOptions,
  SentinelResolver,
} from "./sentinel.js";
import { parseConnectionString } from "./url.js";

export type {
  GeoPosition,
  RedisValue,
  ScanOptions,
  ScanPage,
  SetOptions,
  StreamEntry,
  TransactionRunner,
} from "./commands.js";
export type { CommandArg, Reply } from "./protocol/resp.js";
export {
  // The drivers — what `connect` takes.
  driver,
  type MessageContext,
  type MessageHandler,
  // For a driver built on this one.
  mixinCommands,
  openConnection,
  parseConnectionString,
  REDIS_DIALECT,
  type ReconnectOptions,
  RedisBatch,
  RedisCluster,
  type RedisClusterOptions,
  RedisCommands,
  // The classes they open, for typing and for extending.
  RedisConnection,
  type RedisOptions,
  type RedisPayload,
  RedisPipeline,
  RedisPooled,
  type RedisPoolOptions,
  RedisTransaction,
  redisCluster,
  redisSentinel,
  type SentinelDriverOptions,
  type SentinelOptions,
  SentinelResolver,
  type ServerHello,
};
