/**
 * The driver itself, in a module of its own.
 *
 * `index.ts` would be the natural home, but the cluster and sentinel clients
 * open connections and `index.ts` imports both — so the two would form a cycle.
 * This is the leaf they all depend on instead.
 */
import { type Driver, defineDriver, type PoolSettings } from "runtime:db";

import { RedisConnection, type RedisOptions } from "./connection.js";
import { REDIS, type ServerFamily } from "./family.js";
import { RedisPooled } from "./pool.js";
import { parseConnectionString } from "./url.js";

/** Opens one connection. The driver's `open`, and what the pool's slots call. */
export async function openConnection(
  url: string,
  options: RedisOptions = {},
  family: ServerFamily = REDIS,
): Promise<RedisConnection> {
  const connection = new RedisConnection(family);
  await connection.open(parseConnectionString(url, options, family));
  return connection;
}

/** One driver per family, so the cluster's node pools and `connect` share it. */
const drivers = new WeakMap<ServerFamily, Driver<RedisConnection, RedisOptions, RedisPooled>>();

/**
 * The connection driver for a family: what {@link driver} is for Redis.
 *
 * For a package built on this one — `@opentf/esrun-valkey` is this with
 * Valkey's names. The same family always gives back the same driver.
 */
export function createDriver(
  family: ServerFamily,
): Driver<RedisConnection, RedisOptions, RedisPooled> {
  let made = drivers.get(family);
  if (made === undefined) {
    made = defineFamilyDriver(family);
    drivers.set(family, made);
  }
  return made;
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
export const driver: Driver<RedisConnection, RedisOptions, RedisPooled> = createDriver(REDIS);

function defineFamilyDriver(
  family: ServerFamily,
): Driver<RedisConnection, RedisOptions, RedisPooled> {
  const made = defineDriver<RedisConnection, RedisOptions, RedisPooled>({
    name: family.name,
    schemes: [...family.schemes],
    dialect: family.dialect,
    open: (url: string, options: RedisOptions = {}) => openConnection(url, options, family),
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
      const settings = parseConnectionString(url, options, family);
      return new RedisPooled(made, url, { ...settings, blocking: false }, poolOptions);
    },
  });
  return made;
}
