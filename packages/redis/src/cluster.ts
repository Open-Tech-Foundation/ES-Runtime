/**
 * A cluster client: many nodes, 16384 slots, and the redirects that keep the
 * two in step.
 *
 * The design rests on one observation: **routing is an optimization, and
 * correctness comes from following redirects.** A cluster tells a client that
 * went to the wrong node, by name and with the right address, so a client that
 * guesses badly is slow rather than wrong. That is why the key-extraction table
 * below can be modest and honest about its gaps instead of having to encode
 * every command Redis has ever shipped.
 *
 * What a cluster cannot forgive is a command whose keys are in **different
 * slots**: there is no node that owns both, so `CROSSSLOT` is a refusal rather
 * than a redirect. Hash tags (`{user:1}:name`) are how an application says that
 * some keys must stay together.
 */
import {
  DbError,
  DbErrorCode,
  defineDriver,
  queryAst,
  type CallOptions,
  type DbParams,
  type ExecuteResult,
  type Queryable,
  type Rows,
} from "runtime:db";

import { RedisCommands } from "./commands.js";
import { driver, openConnection } from "./driver.js";
import { REDIS_DIALECT, type RedisConnection, type RedisOptions } from "./connection.js";
import { RedisPooled, type RedisPoolOptions } from "./pool.js";
import type { Redirect } from "./protocol/errors.js";
import type { CommandArg } from "./protocol/resp.js";
import { SLOTS, hashSlot } from "./protocol/slots.js";
import { parseConnectionString } from "./url.js";

export interface RedisClusterOptions extends RedisPoolOptions {
  /**
   * More seed nodes, as `redis://host:port` URLs.
   *
   * The connection string is the first seed. One is enough — the topology is
   * read from the cluster itself — but naming several means the client can
   * still start when one of them is down, which is the situation a cluster
   * exists for.
   */
  seeds?: readonly string[];
  /**
   * How many redirects to follow before giving up. Default 16.
   *
   * A bound rather than a retry count: a cluster mid-resharding legitimately
   * sends a few, and a cluster that is misconfigured can send them in a circle.
   */
  maxRedirects?: number;
}

/**
 * The command a `query`/`execute` argument carries.
 *
 * A bare array or a `queryAst(…)`; SQL text is refused with the same code the
 * single-connection backend refuses it with, because it is the same refusal —
 * Redis has no SQL, in a cluster or out of one.
 */
function commandOf(q: unknown): readonly CommandArg[] {
  if (Array.isArray(q)) return q as readonly CommandArg[];
  if (typeof q === "object" && q !== null && (q as { __queryAst?: boolean }).__queryAst === true) {
    return (q as { ast: readonly CommandArg[] }).ast;
  }
  throw new DbError(
    "the redis backend takes a query AST, not SQL text — build one with queryAst()",
    { code: DbErrorCode.QueryForm },
  );
}

/** A node's address, as the cluster spells it. */
type NodeKey = string;

export class RedisCluster extends RedisCommands {
  readonly #seeds: RedisOptions[];
  readonly #options: RedisClusterOptions;
  readonly #pools = new Map<NodeKey, RedisPooled>();
  /** slot → node. Empty until the first refresh. */
  #slots: (NodeKey | undefined)[] = new Array(SLOTS);
  /** Every node the topology named, whether or not one has been dialled. */
  #known = new Set<NodeKey>();
  #refreshing: Promise<void> | null = null;
  #closed = false;

  constructor(seeds: RedisOptions[], options: RedisClusterOptions = {}) {
    super();
    if (seeds.length === 0) {
      throw new DbError("a cluster needs at least one seed node", {
        code: DbErrorCode.Unsupported,
      });
    }
    this.#seeds = seeds;
    this.#options = options;
  }

  /**
   * Connects to a cluster, given one or more seed nodes.
   *
   * One is enough — the topology is read from the cluster itself — but naming
   * several means the client can still start when one of them is down, which is
   * the situation a cluster exists for.
   */
  static async connect(
    urls: string | readonly string[],
    options: RedisClusterOptions = {},
  ): Promise<RedisCluster> {
    const list = typeof urls === "string" ? [urls] : [...urls];
    const cluster = new RedisCluster(
      list.map((url) => parseConnectionString(url, options)),
      options,
    );
    await cluster.refresh();
    return cluster;
  }

  /**
   * The primaries the topology names, as `host:port`.
   *
   * What the cluster said, not what has been dialled — pools are opened on
   * first use, so reporting those would say "none" for a cluster that is
   * perfectly well understood and merely idle.
   */
  get nodes(): string[] {
    return [...this.#known];
  }

  /** Which node owns a slot, once the topology is known. */
  nodeForSlot(slot: number): string | undefined {
    return this.#slots[slot];
  }

  // -- topology -------------------------------------------------------------

  /**
   * Re-reads the slot map from whichever seed or known node answers first.
   *
   * Concurrent callers share one refresh: a burst of `MOVED`s after a failover
   * should re-read the topology once, not once per command in flight.
   */
  async refresh(): Promise<void> {
    this.#refreshing ??= this.#refresh().finally(() => {
      this.#refreshing = null;
    });
    await this.#refreshing;
  }

  async #refresh(): Promise<void> {
    // Known nodes first: they are proven reachable, where a seed may be the one
    // that just failed. Seeds are the fallback, and the reason a cluster whose
    // every known node has moved can still be found again.
    const candidates: RedisOptions[] = [
      ...[...this.#pools.keys()].map((key) => this.#optionsFor(key)),
      ...this.#seeds,
    ];
    let last: unknown = null;
    for (const target of candidates) {
      let connection: RedisConnection | null = null;
      try {
        connection = await openConnection(hostPort(target), target);
        const reply = (await connection.call(["CLUSTER", "SLOTS"])) as unknown[];
        this.#apply(reply);
        return;
      } catch (e) {
        last = e;
      } finally {
        await connection?.close().catch(() => {});
      }
    }
    throw new DbError(
      `none of the cluster's nodes answered CLUSTER SLOTS (tried ${candidates.length})`,
      { code: DbErrorCode.ConnectionLost, cause: last },
    );
  }

  /**
   * Rebuilds the slot map from a `CLUSTER SLOTS` reply.
   *
   * `[[start, end, [ip, port, id, …], …replicas], …]`, where the first node in
   * each range is the primary. Replicas are ignored: this client sends
   * everything to primaries, because a replica may be behind and nothing here
   * knows which reads could tolerate that.
   */
  #apply(reply: unknown[]): void {
    const slots: (NodeKey | undefined)[] = new Array(SLOTS);
    const seen = new Set<NodeKey>();
    for (const range of reply) {
      if (!Array.isArray(range) || range.length < 3) continue;
      const start = Number(range[0]);
      const end = Number(range[1]);
      const primary = range[2];
      if (!Array.isArray(primary)) continue;
      const host = String(primary[0]);
      const port = Number(primary[1]);
      if (host === "" || !Number.isInteger(port)) continue;
      const key = `${host}:${port}`;
      seen.add(key);
      for (let slot = start; slot <= end && slot < SLOTS; slot++) slots[slot] = key;
    }
    if (seen.size === 0) {
      throw new DbError("CLUSTER SLOTS described no nodes — is this server in cluster mode?", {
        code: DbErrorCode.Unsupported,
      });
    }
    this.#slots = slots;
    this.#known = seen;

    // Pools for nodes that have gone are closed rather than left holding
    // sockets to a server that is no longer part of anything.
    for (const [key, pool] of [...this.#pools]) {
      if (seen.has(key)) continue;
      this.#pools.delete(key);
      void pool.close().catch(() => {});
    }
  }

  #optionsFor(key: NodeKey): RedisOptions {
    const colon = key.lastIndexOf(":");
    const base = this.#seeds[0] ?? {};
    return {
      ...base,
      host: key.slice(0, colon),
      port: Number(key.slice(colon + 1)),
      // Each node is reached through the pool below, which is where the
      // blocking rule already applies.
      blocking: false,
    };
  }

  #poolFor(key: NodeKey): RedisPooled {
    let pool = this.#pools.get(key);
    if (pool === undefined) {
      const target = this.#optionsFor(key);
      pool = new RedisPooled(driver, hostPort(target), target, this.#options);
      this.#pools.set(key, pool);
    }
    return pool;
  }

  /** Any node at all, for a command that names no key. */
  #anyPool(): RedisPooled {
    for (const key of this.#slots) {
      if (key !== undefined) return this.#poolFor(key);
    }
    const seed = this.#seeds[0]!;
    return this.#poolFor(`${seed.host ?? "localhost"}:${seed.port ?? 6379}`);
  }

  #poolForCommand(args: readonly CommandArg[]): RedisPooled {
    const key = routingKey(args);
    if (key === null) return this.#anyPool();
    const node = this.#slots[hashSlot(key)];
    return node === undefined ? this.#anyPool() : this.#poolFor(node);
  }

  // -- the runtime:db half --------------------------------------------------
  //
  // A cluster client is a `Connection` like the rest, so a caller that took one
  // from `connect()` can use `query`/`execute` without knowing whether it is
  // talking to one node or twelve. Each call is routed by its key and run on
  // the node that owns it, through the same redirect-following path the command
  // surface uses.

  readonly dialect = REDIS_DIALECT;
  readonly backend = "redis-cluster";

  /** A command read as rows, on whichever node owns its key. */
  query(q: Queryable | readonly CommandArg[], params?: DbParams, options?: CallOptions): Promise<Rows> {
    const args = commandOf(q);
    return this.#follow(args, (pool) => pool.query(queryAst(args), params, options));
  }

  execute(
    q: Queryable | readonly CommandArg[],
    params?: DbParams,
    options?: CallOptions,
  ): Promise<ExecuteResult> {
    const args = commandOf(q);
    return this.#follow(args, (pool) => pool.execute(queryAst(args), params, options));
  }

  /**
   * One command against many argument sets.
   *
   * A loop rather than a batch, and it has to be: the sets may hash to
   * different slots, so there is no one node to send them to. Each lands where
   * its key belongs.
   */
  async executeMany(
    q: Queryable | readonly CommandArg[],
    rows: readonly DbParams[],
  ): Promise<ExecuteResult> {
    const args = commandOf(q);
    let changes = 0;
    for (const row of rows) {
      const result = await this.execute(args, row);
      changes += result.changes;
    }
    return { changes, lastInsertRowid: null };
  }

  /** A cluster subscribes to nothing: pub/sub here is not cluster-aware. */
  get subscribed(): boolean {
    return false;
  }

  get subscriptions(): string[] {
    return [];
  }

  /**
   * Refused, and by name.
   *
   * Redis pub/sub is not cluster-aware: a message published to one node is not
   * seen by a subscriber on another, so a cluster-wide `subscribe` would
   * deliver some messages and silently miss others — the worst of the available
   * behaviours. Subscribe to a specific node instead, or use `ssubscribe` on a
   * sharded channel, where the slot decides the node and the guarantee holds.
   */
  subscribe(): Promise<void> {
    return Promise.reject(
      new DbError(
        "Redis pub/sub is not cluster-aware: subscribe to one node's connection, or use a sharded channel with ssubscribe",
        { code: DbErrorCode.Unsupported },
      ),
    );
  }

  unsubscribe(): Promise<void> {
    return this.subscribe();
  }

  /** Usable until closed. */
  get usable(): boolean {
    return !this.#closed;
  }

  /** A cluster hands out nothing that could come back unfit; it holds pools. */
  get reusable(): boolean {
    return this.usable;
  }

  /**
   * Refused, and by name — a cluster has no single session to lend.
   *
   * Every other `Connection` can promise that what runs inside `fn` runs on one
   * connection. A cluster cannot: the keys touched inside it may live on
   * different nodes, and the whole point of the client is that it sends each
   * one where it belongs. Refusing is the honest answer, and it is the same
   * answer `transaction` gives for the same reason. For state that must sit on
   * one node — a `WATCH`, a `SELECT` — reach that node's pool with a key that
   * hashes to it.
   */
  withConnection<T>(_fn: (connection: never) => Promise<T>): Promise<T> {
    return Promise.reject(
      new DbError(
        "a cluster has no single connection to hold: its keys may live on different nodes — run the affected keys through one slot, or open a connection to that node",
        { code: DbErrorCode.Unsupported },
      ),
    );
  }

  /**
   * Refused, and by name.
   *
   * `REDIS_DIALECT` already declares that Redis has no transactions in the
   * sense `transaction(fn)` promises. A cluster adds a second reason: the
   * commands in a body may belong to different nodes, and there is no node that
   * could hold them together. `multi()` is the honest thing, and it stays
   * within one slot.
   */
  transaction<T>(_fn: (tx: never) => Promise<T>): Promise<T> {
    return Promise.reject(
      new DbError(
        "a cluster has no transactions: the keys in one may belong to different nodes — use multi() within a single slot",
        { code: DbErrorCode.Unsupported },
      ),
    );
  }

  // -- running commands -----------------------------------------------------

  override async call(
    args: readonly CommandArg[],
    options: { signal?: AbortSignal } = {},
  ): Promise<unknown> {
    return this.#follow(args, (pool, extra) =>
      extra.length === 0
        ? pool.call(args, options)
        : pool.withConnection(async (connection) => {
            // `ASKING` and the command must travel on the *same* connection —
            // the flag it sets lasts exactly one command — so this borrows a
            // connection rather than making two pooled calls.
            for (const prelude of extra) await connection.call(prelude);
            return connection.call(args, options);
          }),
    );
  }

  /**
   * Runs `work` against the node a command belongs on, following redirects.
   *
   * `MOVED` means the slot has moved for good, so the map is re-read; `ASK`
   * means only this command goes elsewhere, and it must be preceded by `ASKING`
   * on the same connection — the difference matters, because treating an `ASK`
   * as a `MOVED` during a resharding would point every later key at a node that
   * does not own it yet.
   */
  async #follow<T>(
    args: readonly CommandArg[],
    work: (pool: RedisPooled, prelude: readonly (readonly CommandArg[])[]) => Promise<T>,
  ): Promise<T> {
    this.#open();
    const limit = this.#options.maxRedirects ?? 16;
    let pool = this.#poolForCommand(args);
    let prelude: readonly (readonly CommandArg[])[] = [];

    for (let hop = 0; ; hop++) {
      try {
        return await work(pool, prelude);
      } catch (e) {
        const redirect = redirectOf(e);
        if (redirect === null) throw e;
        if (hop >= limit) {
          throw new DbError(
            `the cluster redirected this command ${limit} times without settling — the topology may be changing faster than it can be followed, or a slot may be pointing in a circle`,
            { code: DbErrorCode.Unsupported, cause: e },
          );
        }
        const key = `${redirect.host}:${redirect.port}`;
        if (redirect.kind === "MOVED") {
          // The slot moved for good. Believe it for this slot straight away and
          // re-read the whole map in the background, since one moved slot
          // usually means several did.
          this.#slots[redirect.slot] = key;
          prelude = [];
          void this.refresh().catch(() => {});
        } else {
          prelude = [["ASKING"]];
        }
        pool = this.#poolFor(key);
      }
    }
  }

  /**
   * Runs a transaction on the one node that owns its keys.
   *
   * The check comes first because a transaction spanning two slots is not
   * something any node could run, so refusing before sending is both faster and
   * a better message than `CROSSSLOT` would be.
   */
  override async execTransaction(
    commands: readonly (readonly CommandArg[])[],
  ): Promise<unknown[] | null> {
    this.#singleSlot(commands, "transaction");
    // Routed by the first command that names a key, so a leading keyless
    // command does not send the whole transaction to an arbitrary node.
    const routing = commands.find((args) => routingKey(args) !== null) ?? commands[0] ?? ["PING"];
    return this.#follow(routing, (pool) => pool.execTransaction(commands));
  }

  override async execPipeline(commands: readonly (readonly CommandArg[])[]): Promise<unknown[]> {
    this.#open();
    if (commands.length === 0) return [];
    // Grouped by node so each group is still one round trip, and the groups run
    // at the same time — which is a cluster's own advantage rather than a
    // consolation for having to split.
    const groups = new Map<RedisPooled, number[]>();
    for (let i = 0; i < commands.length; i++) {
      const pool = this.#poolForCommand(commands[i]!);
      const group = groups.get(pool);
      if (group === undefined) groups.set(pool, [i]);
      else group.push(i);
    }

    const results = new Array<unknown>(commands.length);
    await Promise.all(
      [...groups].map(async ([pool, indexes]) => {
        const replies = await pool.execPipeline(indexes.map((i) => commands[i]!));
        for (let j = 0; j < indexes.length; j++) results[indexes[j]!] = replies[j];
      }),
    );

    // A redirect inside a pipeline cannot be followed in place — the batch has
    // already been sent — so those commands are re-run one at a time, where
    // `call` follows them properly. Rare, and only during a resharding.
    const redirected = results
      .map((result, i) => (redirectOf(result) === null ? -1 : i))
      .filter((i) => i !== -1);
    if (redirected.length > 0) {
      await this.refresh().catch(() => {});
      await Promise.all(
        redirected.map(async (i) => {
          try {
            results[i] = await this.call(commands[i]!);
          } catch (e) {
            results[i] = e;
          }
        }),
      );
    }
    return results;
  }

  /**
   * The slot every command in a batch shares, refusing a batch that spans two.
   *
   * A transaction has to run on one node, and no node owns two slots' worth of
   * keys. The server would say `CROSSSLOT` anyway; saying it here names the
   * fix — hash tags — and does so before anything is sent.
   */
  #singleSlot(commands: readonly (readonly CommandArg[])[], noun: string): number | null {
    let slot: number | null = null;
    for (const args of commands) {
      const key = routingKey(args);
      if (key === null) continue;
      const candidate = hashSlot(key);
      if (slot === null) slot = candidate;
      else if (slot !== candidate) {
        throw new DbError(
          `every key in a ${noun} must live in the same hash slot, and these do not — put the keys that belong together in a hash tag, like {user:1}:name and {user:1}:email`,
          { code: DbErrorCode.Unsupported, backendCode: "CROSSSLOT" },
        );
      }
    }
    return slot;
  }

  #open(): void {
    if (this.#closed) {
      throw new DbError("the cluster client is closed", { code: DbErrorCode.Closed });
    }
  }

  async close(): Promise<void> {
    this.#closed = true;
    const pools = [...this.#pools.values()];
    this.#pools.clear();
    await Promise.all(pools.map((pool) => pool.close().catch(() => {})));
  }

  async [Symbol.asyncDispose](): Promise<void> {
    await this.close();
  }
}

function hostPort(options: RedisOptions): string {
  const scheme = options.tls === true ? "rediss" : "redis";
  return `${scheme}://${options.host ?? "localhost"}:${options.port ?? 6379}`;
}

/** The redirect an error (or an error sitting in a result array) carries. */
function redirectOf(value: unknown): Redirect | null {
  if (typeof value !== "object" || value === null) return null;
  const redirect = (value as { redirect?: Redirect }).redirect;
  return redirect === undefined ? null : redirect;
}

// ---------------------------------------------------------------------------
// Which key a command routes by
// ---------------------------------------------------------------------------

/**
 * Commands that name no key, and can go to any node.
 *
 * Not exhaustive, and it does not need to be: a command wrongly treated as
 * having a key is routed by an argument that is not one, which lands it on an
 * arbitrary node — and an arbitrary node is where a keyless command was going
 * anyway. The list is here to skip a pointless hash, not to be a specification.
 */
const NO_KEY = new Set([
  "PING", "ECHO", "INFO", "TIME", "DBSIZE", "COMMAND", "CONFIG", "CLIENT",
  "CLUSTER", "ACL", "MEMORY", "SCRIPT", "FUNCTION", "SELECT", "SWAPDB",
  "FLUSHALL", "FLUSHDB", "SHUTDOWN", "LASTSAVE", "BGSAVE", "BGREWRITEAOF",
  "SAVE", "SLOWLOG", "LATENCY", "REPLICAOF", "SLAVEOF", "DEBUG", "RESET",
  "HELLO", "AUTH", "QUIT", "MULTI", "EXEC", "DISCARD", "UNWATCH", "ASKING",
  "READONLY", "READWRITE", "RANDOMKEY", "SCAN", "KEYS", "WAIT", "PUBSUB",
]);

/**
 * Commands whose first key comes after a `numkeys` count.
 *
 * `EVAL script numkeys key…` is the one that matters: routing it by argument 1
 * would hash the *script text*, which is not a key and would send every script
 * to whichever node that text happened to hash to.
 */
const NUMKEYS_AT = new Map([
  ["EVAL", 2],
  ["EVALSHA", 2],
  ["EVAL_RO", 2],
  ["EVALSHA_RO", 2],
  ["FCALL", 2],
  ["FCALL_RO", 2],
  ["ZUNION", 1],
  ["ZINTER", 1],
  ["ZDIFF", 1],
  ["ZINTERCARD", 1],
  ["SINTERCARD", 1],
  ["LMPOP", 1],
  ["ZMPOP", 1],
  ["BLMPOP", 2],
  ["BZMPOP", 2],
]);

/**
 * The key a command should be routed by, or `null` for "anywhere".
 *
 * Deliberately not a full command table. A wrong guess costs a redirect, which
 * the client follows — so this is an optimization, and being approximately
 * right for every command beats being exactly right for the hundred somebody
 * remembered to list. The cases that are handled specially are the ones where
 * argument 1 is *not* a key and hashing it would scatter related commands.
 */
export function routingKey(args: readonly CommandArg[]): string | null {
  const first = args[0];
  if (typeof first !== "string") return null;
  const name = first.toUpperCase();
  if (NO_KEY.has(name)) return null;

  const numkeysAt = NUMKEYS_AT.get(name);
  if (numkeysAt !== undefined) {
    const count = Number(args[numkeysAt]);
    if (!Number.isInteger(count) || count <= 0) return null;
    const key = args[numkeysAt + 1];
    return key === undefined ? null : String(key);
  }

  if (name === "XREAD" || name === "XREADGROUP") {
    for (let i = 1; i < args.length; i++) {
      const token = args[i];
      if (typeof token === "string" && token.toUpperCase() === "STREAMS") {
        const key = args[i + 1];
        return key === undefined ? null : String(key);
      }
    }
    return null;
  }

  const key = args[1];
  return key === undefined ? null : String(key);
}

/**
 * The cluster driver.
 *
 * ```js
 * import { connect } from "runtime:db";
 * import { redisCluster } from "@opentf/esrun-redis";
 *
 * const cluster = await connect("redis://10.0.0.1:7001", {
 *   driver: redisCluster,
 *   seeds: ["redis://10.0.0.2:7001"],
 * });
 * await cluster.set("user:1", "ada");
 * ```
 *
 * A different driver rather than an option on the first one, because it is a
 * different client: it holds a pool per node and routes by slot, and the thing
 * it returns is a `RedisCluster`. Making that a flag would mean one call whose
 * return type depended on a boolean.
 */
export const redisCluster = defineDriver<RedisCluster, RedisClusterOptions, never>({
  name: "redis-cluster",
  schemes: ["redis", "rediss"],
  dialect: REDIS_DIALECT,
  open(url: string, options: RedisClusterOptions = {}): Promise<RedisCluster> {
    return RedisCluster.connect([url, ...(options.seeds ?? [])], options);
  },
  /**
   * A cluster client already pools — one pool per node, sized by the same
   * options. `pool: true` on top of it would be a second pool over the first,
   * so it is refused by name rather than quietly built.
   */
  pooled(): never {
    throw new DbError(
      "a cluster client already holds a pool per node — pass max/idleTimeout as ordinary options instead of pool",
      { code: DbErrorCode.Unsupported },
    );
  },
});
