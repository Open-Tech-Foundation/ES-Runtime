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
import { RedisConnection, REDIS_DIALECT, type RedisOptions, type ServerHello } from "./connection.js";
import { RedisPool, type RedisPoolOptions } from "./pool.js";
import { parseConnectionString } from "./url.js";

export {
  Redis,
  RedisCommands,
  RedisConnection,
  RedisPool,
  REDIS_DIALECT,
  parseConnectionString,
  type RedisOptions,
  type RedisPoolOptions,
  type ServerHello,
};
export type { CommandArg, Reply } from "./protocol/resp.js";
export type { RedisValue, ScanOptions, ScanPage, SetOptions } from "./commands.js";

/** Opens a connection without going through `runtime:db`'s registry. */
export async function connect(url: string, options: RedisOptions = {}): Promise<RedisConnection> {
  const connection = new RedisConnection();
  await connection.open(parseConnectionString(url, options));
  return connection;
}

/** Opens a client — a connection with the command surface on it. */
export function createClient(url: string, options: RedisOptions = {}): Promise<Redis> {
  return Redis.connect(url, options);
}

/**
 * A pool over the same connection string.
 *
 * Nothing is opened here: connections are made when they are first needed, so a
 * pool costs nothing until something asks it for work.
 */
export function createPool(url: string, options: RedisPoolOptions = {}): RedisPool {
  const settings = parseConnectionString(url, options);
  return new RedisPool(() => connect(url, settings), options);
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
