/**
 * Sentinel: finding the master, and finding it again after it moves.
 *
 * A Sentinel deployment is a set of processes watching one master and its
 * replicas. Clients do not connect to them for data — they ask them *where* the
 * master is, and connect there. When the master fails, the sentinels elect one
 * of the replicas and the answer changes.
 *
 * So the interesting part of a Sentinel client is not the lookup, which is one
 * command. It is that the lookup has to happen **again** at exactly the right
 * moments, and that the answer has to be checked rather than trusted: a
 * sentinel mid-failover will happily hand out the address of a server that has
 * just become a replica, and a client that wrote to it would lose the writes.
 */
import { DbError, DbErrorCode } from "runtime:db";

import { Redis } from "./client.js";
import { connect } from "./connect.js";
import type { RedisOptions } from "./connection.js";
import { RedisPool, type RedisPoolOptions } from "./pool.js";
import { parseConnectionString } from "./url.js";

export interface SentinelOptions extends RedisPoolOptions {
  /** The sentinels to ask, as `redis://host:26379` URLs. */
  sentinels: readonly string[];
  /** The name the sentinels know this master by — `mymaster`, usually. */
  masterName: string;
  /**
   * The sentinels' own password, if they have one.
   *
   * Separate from the data password: a Sentinel deployment often protects the
   * sentinels and the master with different credentials, and reusing one for
   * the other is how a client ends up unable to find a master it could have
   * used perfectly well.
   */
  sentinelPassword?: string;
  sentinelUsername?: string;
  /** How long to spend asking one sentinel before trying the next. Default 1000ms. */
  sentinelTimeout?: number;
}

/** Where the master is, and how to ask again when it moves. */
export class SentinelResolver {
  readonly #options: SentinelOptions;
  /** The sentinels, reordered as they answer — see {@link resolve}. */
  #sentinels: string[];

  constructor(options: SentinelOptions) {
    if (options.sentinels.length === 0) {
      throw new DbError("a sentinel client needs at least one sentinel address", {
        code: DbErrorCode.Unsupported,
      });
    }
    if (!options.masterName) {
      throw new DbError("a sentinel client needs the master's name", {
        code: DbErrorCode.Unsupported,
      });
    }
    this.#options = options;
    this.#sentinels = [...options.sentinels];
  }

  /** The sentinels in the order they will be asked. */
  get sentinels(): string[] {
    return [...this.#sentinels];
  }

  /**
   * Asks the sentinels where the master is.
   *
   * Each in turn until one answers, because a sentinel being down is the
   * ordinary case rather than an exception — that is what there are several of
   * them for. The one that answered is moved to the front, so the next lookup
   * starts with a sentinel known to be up instead of walking the same dead ones
   * again.
   *
   * The address is **verified** before it is returned: a sentinel mid-failover
   * will hand out a server that has just become a replica, and writing to a
   * replica loses the writes silently. `ROLE` is one round trip and it turns a
   * data-loss window into a retry.
   */
  async resolve(): Promise<{ host: string; port: number }> {
    const failures: string[] = [];
    for (let i = 0; i < this.#sentinels.length; i++) {
      const address = this.#sentinels[i]!;
      let sentinel: Redis | null = null;
      try {
        sentinel = await Redis.connect(address, {
          connectTimeout: this.#options.sentinelTimeout ?? 1000,
          ...(this.#options.sentinelPassword === undefined
            ? {}
            : { password: this.#options.sentinelPassword }),
          ...(this.#options.sentinelUsername === undefined
            ? {}
            : { username: this.#options.sentinelUsername }),
        });
        const reply = (await sentinel.call([
          "SENTINEL",
          "get-master-addr-by-name",
          this.#options.masterName,
        ])) as unknown;
        if (!Array.isArray(reply) || reply.length < 2) {
          // A sentinel that has never heard of this master answers null. That
          // is a configuration mistake rather than an outage, and it will read
          // the same from every sentinel, so say so plainly.
          failures.push(`${address}: no master named ${this.#options.masterName}`);
          continue;
        }
        const host = String(reply[0]);
        const port = Number(reply[1]);
        if (host === "" || !Number.isInteger(port)) {
          failures.push(`${address}: answered an address that is not one`);
          continue;
        }
        await this.#verifyMaster(host, port);
        // Front of the queue: it is up, and the next lookup should start here.
        if (i !== 0) {
          this.#sentinels.splice(i, 1);
          this.#sentinels.unshift(address);
        }
        return { host, port };
      } catch (e) {
        failures.push(`${address}: ${e instanceof Error ? e.message : String(e)}`);
      } finally {
        await sentinel?.close().catch(() => {});
      }
    }
    throw new DbError(
      `no sentinel could give the address of "${this.#options.masterName}" — ${failures.join("; ")}`,
      { code: DbErrorCode.ConnectionLost },
    );
  }

  /** Refuses an address that is not actually a master any more. */
  async #verifyMaster(host: string, port: number): Promise<void> {
    const candidate = await connect(`redis://${host}:${port}`, {
      ...dataOptions(this.#options),
      host,
      port,
      connectTimeout: this.#options.sentinelTimeout ?? 1000,
    });
    try {
      // `ROLE` answers `["master", …]` or `["slave", …]`. Redis has not renamed
      // the reply, whatever the documentation now calls it.
      const role = (await candidate.command(["ROLE"])) as unknown;
      const kind = Array.isArray(role) ? String(role[0]) : "";
      if (kind !== "master") {
        throw new DbError(
          `${host}:${port} is a ${kind || "unknown role"}, not the master — the failover has not settled`,
          { code: DbErrorCode.Busy },
        );
      }
    } finally {
      await candidate.close().catch(() => {});
    }
  }
}

/** The options that belong to the data connection rather than to the lookup. */
function dataOptions(options: SentinelOptions): RedisOptions {
  const {
    sentinels: _sentinels,
    masterName: _masterName,
    sentinelPassword: _sentinelPassword,
    sentinelUsername: _sentinelUsername,
    sentinelTimeout: _sentinelTimeout,
    ...rest
  } = options;
  return rest;
}

/**
 * A client that finds its master through Sentinel, and finds it again when it
 * moves.
 *
 * ```js
 * const r = await createSentinelClient({
 *   sentinels: ["redis://10.0.0.1:26379", "redis://10.0.0.2:26379"],
 *   masterName: "mymaster",
 *   reconnect: true,
 * });
 * ```
 *
 * With `reconnect` on, a failover is handled without the caller doing anything:
 * the connection to the old master dies, reopening re-runs the lookup, and the
 * new master is where it lands. Without it, the connection simply reports the
 * loss — which is the same choice reconnection makes everywhere else, for the
 * same reason.
 */
export async function createSentinelClient(options: SentinelOptions): Promise<Redis> {
  const resolver = new SentinelResolver(options);
  const first = await resolver.resolve();
  const connection = await connect(`redis://${first.host}:${first.port}`, {
    ...parseConnectionString(`redis://${first.host}:${first.port}`, dataOptions(options)),
    resolve: () => resolver.resolve(),
  });
  return new Redis(connection);
}

/**
 * A pool whose connections each find the master when they are opened.
 *
 * The shape that survives a failover best, and not by accident: a pool already
 * discards connections that fail, and every replacement resolves again. So the
 * pool converges on the new master by doing the thing it does anyway.
 */
export async function createSentinelPool(options: SentinelOptions): Promise<RedisPool> {
  const resolver = new SentinelResolver(options);
  // Resolved once up front so a misconfiguration — a master nobody has heard
  // of, sentinels that are all down — fails here rather than at the first
  // command, somewhere far from the code that got it wrong.
  await resolver.resolve();
  return new RedisPool(
    () =>
      connect("redis://placeholder", {
        ...dataOptions(options),
        resolve: () => resolver.resolve(),
      }),
    options,
  );
}
