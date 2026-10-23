/**
 * `@opentf/esrun-valkey` — the Valkey driver for `runtime:db`.
 *
 * Valkey forked Redis and kept its protocol, so this package is
 * `@opentf/esrun-redis` under Valkey's names rather than a second
 * implementation of RESP: the same connection, pool, cluster and Sentinel
 * clients, built from the Valkey family. What the family changes is what a URL
 * may be spelled as, and what the server is called in `backend` and in errors.
 *
 * ```js
 * import { connect } from "runtime:db";
 * import { driver } from "@opentf/esrun-valkey";
 *
 * const v = await connect("valkey://localhost", { driver });
 * await v.set("greeting", "hello", { ex: 60 });
 * await v.get("greeting");
 * ```
 *
 * `valkey://` and `valkeys://` (TLS) are Valkey's own spellings; `redis://` and
 * `rediss://` are taken too, as iovalkey takes them, so a connection string
 * written for Redis keeps working when this package replaces that one.
 *
 * See DECISIONS D185.
 */

import type { Driver } from "runtime:db";
import {
  createClusterDriver,
  createDriver,
  createSentinelDriver,
  defineFamily,
  parseConnectionString as parseRespUrl,
  type RedisCluster,
  type RedisClusterOptions,
  type RedisConnection,
  type RedisOptions,
  type RedisPooled,
  type SentinelDriverOptions,
  type ServerFamily,
} from "@opentf/esrun-redis";

/** Valkey: `valkey://` and `valkeys://`, and the Redis spellings beside them. */
export const VALKEY: ServerFamily = defineFamily({
  name: "valkey",
  schemes: ["valkey", "valkeys", "redis", "rediss"],
  tlsSchemes: ["valkeys", "rediss"],
});

/**
 * The Valkey driver: one connection, or a pool with the same surface under
 * `pool: true`.
 *
 * What comes back speaks both vocabularies — Valkey's commands as methods, and
 * `runtime:db`'s `query`/`execute` — on the same connection.
 */
export const driver: Driver<RedisConnection, RedisOptions, RedisPooled> = createDriver(VALKEY);

/** The cluster driver: a pool per node, routed by slot, following redirects. */
export const valkeyCluster: Driver<RedisCluster, RedisClusterOptions, never> =
  createClusterDriver(VALKEY);

/** The Sentinel driver: finds the primary through Sentinel, and again after a failover. */
export const valkeySentinel: Driver<RedisConnection, SentinelDriverOptions, RedisPooled> =
  createSentinelDriver(VALKEY);

/** A `valkey://` connection string, as options. */
export function parseConnectionString(url: string, overrides: RedisOptions = {}): RedisOptions {
  return parseRespUrl(url, overrides, VALKEY);
}

// The classes are the Redis package's own, so `instanceof` holds under either
// name; the Valkey names are what code written for Valkey reads as.
export type {
  CommandArg,
  GeoPosition,
  MessageContext,
  MessageHandler,
  ReconnectOptions,
  RedisClusterOptions as ValkeyClusterOptions,
  RedisOptions as ValkeyOptions,
  RedisPayload as ValkeyPayload,
  RedisPoolOptions as ValkeyPoolOptions,
  RedisValue as ValkeyValue,
  Reply,
  ScanOptions,
  ScanPage,
  SentinelDriverOptions,
  SentinelOptions,
  ServerFamily,
  ServerHello,
  SetOptions,
  StreamEntry,
  TransactionRunner,
} from "@opentf/esrun-redis";
export {
  RedisBatch as ValkeyBatch,
  RedisCluster as ValkeyCluster,
  RedisConnection as ValkeyConnection,
  RedisPipeline as ValkeyPipeline,
  RedisPooled as ValkeyPooled,
  RedisTransaction as ValkeyTransaction,
} from "@opentf/esrun-redis";
