/**
 * `@opentf/esrun-redis` — a Redis backend for `runtime:db`, and a Redis client.
 *
 * Importing this package registers the `redis:` and `rediss:` schemes:
 *
 * ```js
 * import "@opentf/esrun-redis";
 * import { connect, queryAst } from "runtime:db";
 *
 * const db = await connect("redis://localhost");
 * await db.execute(queryAst(["SET", "greeting", "hello"]));
 * const rows = await db.query(queryAst(["LRANGE", "log", 0, -1]));
 * ```
 *
 * Most code wants the client instead, which is the same connection with Redis's
 * own vocabulary on it:
 *
 * ```js
 * import { Redis } from "@opentf/esrun-redis";
 *
 * const r = await Redis.connect("redis://localhost");
 * await r.set("greeting", "hello", { ex: 60 });
 * await r.get("greeting");
 * ```
 *
 * There is no native code here, and none was added to the runtime for it. The
 * driver is JavaScript over `runtime:net` — the arrangement D56 committed to,
 * in the sentence that names Redis by name.
 */
import { registerBackend } from "runtime:db";

import { RedisCommands } from "./commands.js";
import { Redis } from "./client.js";
import {
  RedisConnection,
  REDIS_DIALECT,
  type MessageContext,
  type MessageHandler,
  type ReconnectOptions,
  type RedisOptions,
  type RedisPayload,
  type ServerHello,
} from "./connection.js";
import { RedisPool, type RedisPoolOptions } from "./pool.js";
import { RedisBatch, RedisPipeline, RedisTransaction } from "./batch.js";
import { RedisCluster, type RedisClusterOptions } from "./cluster.js";
import { connect } from "./connect.js";
import {
  SentinelResolver,
  createSentinelClient,
  createSentinelPool,
  type SentinelOptions,
} from "./sentinel.js";
import { parseConnectionString } from "./url.js";

export {
  Redis,
  RedisCommands,
  RedisConnection,
  RedisBatch,
  RedisCluster,
  RedisPipeline,
  RedisPool,
  RedisTransaction,
  REDIS_DIALECT,
  SentinelResolver,
  createSentinelClient,
  createSentinelPool,
  parseConnectionString,
  type MessageContext,
  type MessageHandler,
  type ReconnectOptions,
  type RedisOptions,
  type RedisPayload,
  type RedisClusterOptions,
  type RedisPoolOptions,
  type SentinelOptions,
  type ServerHello,
};
export type { CommandArg, Reply } from "./protocol/resp.js";
export type {
  GeoPosition,
  RedisValue,
  ScanOptions,
  ScanPage,
  SetOptions,
  StreamEntry,
  TransactionRunner,
} from "./commands.js";

export { connect };

/** Opens a client — a connection with the command surface on it. */
export function createClient(url: string, options: RedisOptions = {}): Promise<Redis> {
  return Redis.connect(url, options);
}

/**
 * Opens a client meant for subscribing.
 *
 * The same `Redis` as {@link createClient} — the name is the documentation.
 * Subscribing gives a connection over to a read loop and it runs no ordinary
 * commands afterwards, so a program that both listens and works needs two
 * connections, and saying which is which where they are opened is how that
 * stops being a surprise later.
 */
export function createSubscriber(url: string, options: RedisOptions = {}): Promise<Redis> {
  return Redis.connect(url, options);
}

/**
 * Connects to a cluster, given one or more seed nodes.
 *
 * ```js
 * const cluster = await createCluster(["redis://10.0.0.1:7001", "redis://10.0.0.2:7001"]);
 * await cluster.set("user:1", "ada");
 * ```
 *
 * One seed is enough — the topology is read from the cluster itself — but
 * naming several means the client can still start when one of them is down,
 * which is the situation a cluster exists for.
 */
export function createCluster(
  urls: string | readonly string[],
  options: RedisClusterOptions = {},
): Promise<RedisCluster> {
  return RedisCluster.connect(urls, options);
}

/**
 * A pool over the same connection string.
 *
 * Nothing is opened here: connections are made when they are first needed, so a
 * pool costs nothing until something asks it for work.
 */
export function createPool(url: string, options: RedisPoolOptions = {}): RedisPool {
  const settings = parseConnectionString(url, options);
  // `blocking` is stripped rather than honoured. A pool's whole premise is that
  // its connections come back, and a command that blocks indefinitely is one
  // that never returns its connection — so a pool built with the option would
  // hand out connections that can be taken out of circulation permanently,
  // which is the failure the option exists to make deliberate. Blocking
  // indefinitely needs a connection of its own, by construction.
  return new RedisPool(() => connect(url, { ...settings, blocking: false }), options);
}

for (const scheme of ["redis", "rediss"]) {
  registerBackend(scheme, (url, options) => {
    // `connect("redis://…", { pool: true })` through `runtime:db` gives a pool,
    // since a driver's own entry point should not be the only way to reach one.
    const pool = (options as { pool?: boolean | RedisPoolOptions }).pool;
    if (pool !== undefined && pool !== false) {
      return Promise.resolve(
        createPool(url, { ...(options as RedisPoolOptions), ...(pool === true ? {} : pool) }),
      ) as Promise<never>;
    }
    return connect(url, options as RedisOptions);
  });
}
