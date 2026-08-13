/**
 * The driver itself, in a module of its own.
 *
 * `index.ts` would be the natural home, but the cluster and sentinel clients
 * open connections and `index.ts` imports both — so the two would form a cycle.
 * This is the leaf they all depend on instead.
 */
import { type Driver, defineDriver, type PoolSettings } from "runtime:db";

import { REDIS_DIALECT, RedisConnection, type RedisOptions } from "./connection.js";
import { RedisPooled } from "./pool.js";
import { parseConnectionString } from "./url.js";

/** Opens one connection. The driver's `open`, and what the pool's slots call. */
export async function openConnection(
  url: string,
  options: RedisOptions = {},
): Promise<RedisConnection> {
  const connection = new RedisConnection();
  await connection.open(parseConnectionString(url, options));
  return connection;
}

/**
 * The Redis driver.
 *
 * ```js
 * import { connect } from "runtime:db";
 * import { driver } from "@opentf/esrun-redis";
 *
 * const r = await connect("redis://localhost", { driver });
 * await r.set("greeting", "hello", { ex: 60 });
 * ```
 *
 * What comes back speaks both vocabularies: Redis's own commands, and
 * `runtime:db`'s `query`/`execute` for anything portable. They are the same
 * connection, so there is nothing to choose between at the point of opening it.
 */
export const driver: Driver<RedisConnection, RedisOptions, RedisPooled> = defineDriver<
  RedisConnection,
  RedisOptions,
  RedisPooled
>({
  name: "redis",
  schemes: ["redis", "rediss"],
  dialect: REDIS_DIALECT,
  open: openConnection,
  /**
   * Nothing is opened here: connections are made when they are first needed, so
   * a pool costs nothing until something asks it for work.
   *
   * `blocking` is stripped rather than honoured. A pool's whole premise is that
   * its connections come back, and a command that blocks indefinitely is one
   * that never returns its connection — so a pool built with the option would
   * hand out connections that can be taken out of circulation permanently,
   * which is the failure the option exists to make deliberate. Blocking
   * indefinitely needs a connection of its own, by construction.
   */
  pooled(url: string, options: RedisOptions = {}, poolOptions: PoolSettings = {}): RedisPooled {
    const settings = parseConnectionString(url, options);
    return new RedisPooled(driver, url, { ...settings, blocking: false }, poolOptions);
  },
});
