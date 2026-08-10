/**
 * Opening a connection, in a module of its own.
 *
 * `index.ts` would be the natural home, but the cluster client needs to open
 * connections and `index.ts` imports the cluster client — so the two would form
 * a cycle. This is the leaf they both depend on instead.
 */
import { RedisConnection, type RedisOptions } from "./connection.js";
import { parseConnectionString } from "./url.js";

/** Opens a connection without going through `runtime:db`'s registry. */
export async function connect(url: string, options: RedisOptions = {}): Promise<RedisConnection> {
  const connection = new RedisConnection();
  await connection.open(parseConnectionString(url, options));
  return connection;
}
