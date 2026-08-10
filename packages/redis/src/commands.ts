/**
 * The command surface: Redis as Redis, rather than Redis pretending to be a
 * database with rows in it.
 *
 * This is the half of the package most people will use. `runtime:db`'s
 * `connect()` / `query()` still reaches Redis — that is what makes it a
 * backend, and what an ORM or a portable tool would target — but nobody wants
 * to write `db.query(queryAst(["HGETALL", key]))` when they mean `hgetall`.
 *
 * Everything is `call()` underneath, so there is exactly one place a command
 * turns into bytes, and anything without a helper here is still reachable:
 * `client.call(["OBJECT", "ENCODING", key])`. The helpers are a convenience
 * over a complete surface rather than the surface itself, which is why the ones
 * this release omits — streams, geo, bitmaps, HyperLogLog — cost a caller
 * nothing but a little typing.
 *
 * It is abstract so that a single connection and a pool present the same
 * methods without either of them reimplementing eighty of them.
 */
import type { CommandArg } from "./protocol/resp.js";
import type { RedisPipeline, RedisTransaction } from "./batch.js";

/** What can run a built transaction — a client, or a pool. */
export interface TransactionRunner {
  execTransaction(commands: readonly (readonly CommandArg[])[]): Promise<unknown[] | null>;
  execPipeline(commands: readonly (readonly CommandArg[])[]): Promise<unknown[]>;
}

/** The value a Redis key holds. `Uint8Array` in `binary` mode. */
export type RedisValue = string | Uint8Array;

/** Options for `SET`. */
export interface SetOptions {
  /** Expire after this many seconds. */
  ex?: number;
  /** Expire after this many milliseconds. */
  px?: number;
  /** Expire at this Unix time, in seconds. */
  exat?: number;
  /** Expire at this Unix time, in milliseconds. */
  pxat?: number;
  /** Only set if the key does not exist. */
  nx?: boolean;
  /** Only set if the key already exists. */
  xx?: boolean;
  /** Keep the existing TTL rather than clearing it. */
  keepttl?: boolean;
  /** Return the old value instead of the status. */
  get?: boolean;
}

/** One entry in a stream: its id, and its field/value pairs. */
export interface StreamEntry {
  /** `1699999999999-0` — a millisecond timestamp and a sequence number. */
  id: string;
  fields: Record<string, RedisValue>;
}

/** A point on the earth, as Redis stores it. */
export interface GeoPosition {
  longitude: number;
  latitude: number;
}

/** What `SCAN` and its relatives answer: a cursor to continue from, and a page. */
export interface ScanPage<T> {
  /** `"0"` when the iteration is complete. */
  cursor: string;
  items: T[];
}

export interface ScanOptions {
  match?: string;
  count?: number;
  /** `SCAN` only: restrict to one value type, e.g. `"hash"`. */
  type?: string;
}

export abstract class RedisCommands {
  /**
   * Runs one command and returns its reply as ordinary JavaScript.
   *
   * The one method an implementation supplies. Everything else in this class is
   * built from it, so a pool, a single connection and anything added later
   * agree on what a command does by construction.
   */
  abstract call(args: readonly CommandArg[], options?: { signal?: AbortSignal }): Promise<unknown>;

  /**
   * Runs a built transaction. Supplied by whatever owns a connection.
   *
   * On {@link RedisTransaction} itself this is not reachable — a transaction
   * inside a transaction is not a thing Redis has — so it refuses.
   */
  abstract execTransaction(commands: readonly (readonly CommandArg[])[]): Promise<unknown[] | null>;

  /** Runs a built pipeline. Supplied by whatever owns a connection. */
  abstract execPipeline(commands: readonly (readonly CommandArg[])[]): Promise<unknown[]>;

  // -- MULTI/EXEC -----------------------------------------------------------

  /**
   * Starts a transaction.
   *
   * ```js
   * const tx = r.multi();
   * tx.set("a", "1");
   * const n = tx.incr("counter");
   * const [, counter] = await tx.exec();
   * ```
   *
   * Not `transaction(fn)`: `MULTI`/`EXEC` applies its commands together with
   * nothing interleaved, but does **not** roll back one that fails at exec
   * time. See {@link RedisTransaction.exec}.
   */
  multi(): RedisTransaction {
    // Required lazily: `batch.ts` extends this class, so importing it at the top
    // would be a cycle that leaves one of the two undefined at construction
    // time depending on which module the loader reached first.
    return new (batches().transaction)(this);
  }

  /**
   * Starts a pipeline: many commands, one round trip, no atomicity.
   *
   * ```js
   * const p = r.pipeline();
   * for (const id of ids) p.hgetall(`user:${id}`);
   * const users = await p.exec();
   * ```
   *
   * The reason to reach for it is arithmetic rather than taste. A command costs
   * a round trip whatever it carries, so a loop of `await`s spends its time on
   * the network rather than in Redis; a pipeline pays for one.
   *
   * It is **not** a transaction. Another client's commands may land among these,
   * and one failing does not stop the rest — {@link multi} is the one that asks
   * the server for isolation.
   */
  pipeline(): RedisPipeline {
    return new (batches().pipeline)(this);
  }

  // -- strings --------------------------------------------------------------

  async get(key: string): Promise<RedisValue | null> {
    return (await this.call(["GET", key])) as RedisValue | null;
  }

  /**
   * `SET`, with its options spelled as an object.
   *
   * Returns the status (`"OK"`), or `null` when `nx`/`xx` meant it did not
   * apply — which is the whole point of those flags, and the reason this does
   * not throw on a no-op. With `get: true` it returns the **old** value
   * instead, and `null` then means the key did not exist.
   */
  async set(key: string, value: CommandArg, options: SetOptions = {}): Promise<RedisValue | null> {
    const args: CommandArg[] = ["SET", key, value];
    if (options.ex !== undefined) args.push("EX", options.ex);
    if (options.px !== undefined) args.push("PX", options.px);
    if (options.exat !== undefined) args.push("EXAT", options.exat);
    if (options.pxat !== undefined) args.push("PXAT", options.pxat);
    if (options.keepttl) args.push("KEEPTTL");
    if (options.nx) args.push("NX");
    if (options.xx) args.push("XX");
    if (options.get) args.push("GET");
    return (await this.call(args)) as RedisValue | null;
  }

  async setnx(key: string, value: CommandArg): Promise<boolean> {
    return truthy(await this.call(["SETNX", key, value]));
  }

  async setex(key: string, seconds: number, value: CommandArg): Promise<string> {
    return String(await this.call(["SETEX", key, seconds, value]));
  }

  async getdel(key: string): Promise<RedisValue | null> {
    return (await this.call(["GETDEL", key])) as RedisValue | null;
  }

  /** `GETEX` — read and re-expire in one round trip. No option means "keep". */
  async getex(
    key: string,
    options: { ex?: number; px?: number; exat?: number; pxat?: number; persist?: boolean } = {},
  ): Promise<RedisValue | null> {
    const args: CommandArg[] = ["GETEX", key];
    if (options.ex !== undefined) args.push("EX", options.ex);
    if (options.px !== undefined) args.push("PX", options.px);
    if (options.exat !== undefined) args.push("EXAT", options.exat);
    if (options.pxat !== undefined) args.push("PXAT", options.pxat);
    if (options.persist) args.push("PERSIST");
    return (await this.call(args)) as RedisValue | null;
  }

  async mget(...keys: string[]): Promise<(RedisValue | null)[]> {
    return (await this.call(["MGET", ...keys])) as (RedisValue | null)[];
  }

  /** `MSET`, taking the pairs as an object. */
  async mset(entries: Record<string, CommandArg>): Promise<string> {
    const args: CommandArg[] = ["MSET"];
    for (const [key, value] of Object.entries(entries)) args.push(key, value);
    return String(await this.call(args));
  }

  async append(key: string, value: CommandArg): Promise<number> {
    return count(await this.call(["APPEND", key, value]));
  }

  async strlen(key: string): Promise<number> {
    return count(await this.call(["STRLEN", key]));
  }

  /**
   * `INCR` and friends return the **new** value, not the delta.
   *
   * They come back as `number` where one holds the value exactly and `bigint`
   * where it does not: Redis counters are signed 64-bit, and a counter past
   * 2^53 is a thing that happens.
   */
  async incr(key: string): Promise<number | bigint> {
    return integer(await this.call(["INCR", key]));
  }

  async incrBy(key: string, by: number | bigint): Promise<number | bigint> {
    return integer(await this.call(["INCRBY", key, by]));
  }

  /** Returns a `number`: Redis answers this one as a string, and it is a float. */
  async incrByFloat(key: string, by: number): Promise<number> {
    return Number(await this.call(["INCRBYFLOAT", key, by]));
  }

  async decr(key: string): Promise<number | bigint> {
    return integer(await this.call(["DECR", key]));
  }

  async decrBy(key: string, by: number | bigint): Promise<number | bigint> {
    return integer(await this.call(["DECRBY", key, by]));
  }

  // -- keys -----------------------------------------------------------------

  /** How many of the named keys were removed. */
  async del(...keys: string[]): Promise<number> {
    return count(await this.call(["DEL", ...keys]));
  }

  /** `DEL`, with the reclaiming done on another thread. */
  async unlink(...keys: string[]): Promise<number> {
    return count(await this.call(["UNLINK", ...keys]));
  }

  async exists(...keys: string[]): Promise<number> {
    return count(await this.call(["EXISTS", ...keys]));
  }

  async expire(key: string, seconds: number): Promise<boolean> {
    return truthy(await this.call(["EXPIRE", key, seconds]));
  }

  async pexpire(key: string, milliseconds: number): Promise<boolean> {
    return truthy(await this.call(["PEXPIRE", key, milliseconds]));
  }

  async expireAt(key: string, unixSeconds: number): Promise<boolean> {
    return truthy(await this.call(["EXPIREAT", key, unixSeconds]));
  }

  async persist(key: string): Promise<boolean> {
    return truthy(await this.call(["PERSIST", key]));
  }

  /** Seconds remaining; `-1` with no expiry, `-2` when the key is gone. */
  async ttl(key: string): Promise<number> {
    return count(await this.call(["TTL", key]));
  }

  async pttl(key: string): Promise<number> {
    return count(await this.call(["PTTL", key]));
  }

  /** `"string"`, `"hash"`, …, or `"none"` for a key that is not there. */
  async type(key: string): Promise<string> {
    return String(await this.call(["TYPE", key]));
  }

  async rename(key: string, to: string): Promise<string> {
    return String(await this.call(["RENAME", key, to]));
  }

  async renamenx(key: string, to: string): Promise<boolean> {
    return truthy(await this.call(["RENAMENX", key, to]));
  }

  async touch(...keys: string[]): Promise<number> {
    return count(await this.call(["TOUCH", ...keys]));
  }

  async randomkey(): Promise<string | null> {
    const value = await this.call(["RANDOMKEY"]);
    return value === null ? null : String(value);
  }

  /**
   * `KEYS`, which walks the **whole** keyspace and blocks the server while it
   * does. Use {@link scan} against anything with data in it.
   */
  async keys(pattern: string): Promise<string[]> {
    return ((await this.call(["KEYS", pattern])) as unknown[]).map(String);
  }

  /**
   * One page of `SCAN`.
   *
   * Redis's cursor is not an offset and the pages are not a partition: a key
   * present throughout is returned at least once, and one added or removed
   * during the walk may be returned or not. `count` is a hint about work per
   * call, not a page size — a page may come back empty with a non-zero cursor,
   * which means keep going rather than stop.
   */
  async scan(cursor: string | number = 0, options: ScanOptions = {}): Promise<ScanPage<string>> {
    const args: CommandArg[] = ["SCAN", String(cursor)];
    if (options.match !== undefined) args.push("MATCH", options.match);
    if (options.count !== undefined) args.push("COUNT", options.count);
    if (options.type !== undefined) args.push("TYPE", options.type);
    return page(await this.call(args), (items) => items.map(String));
  }

  /** Every key matching `pattern`, by walking {@link scan} to the end. */
  async *scanIterator(options: ScanOptions = {}): AsyncGenerator<string> {
    let cursor = "0";
    do {
      const result = await this.scan(cursor, options);
      cursor = result.cursor;
      for (const key of result.items) yield key;
    } while (cursor !== "0");
  }

  /**
   * Every field of a hash, by walking {@link hscan} to the end.
   *
   * The reason to prefer this over `hgetall` on a large hash is the same reason
   * `scan` exists: `HGETALL` builds the whole reply on the server and sends it
   * in one go, where this pays for a page at a time.
   */
  async *hscanIterator(key: string, options: ScanOptions = {}): AsyncGenerator<[string, RedisValue]> {
    let cursor = "0";
    do {
      const page = await this.hscan(key, cursor, options);
      cursor = page.cursor;
      for (const entry of page.items) yield entry;
    } while (cursor !== "0");
  }

  /** Every member of a set, a page at a time. */
  async *sscanIterator(key: string, options: ScanOptions = {}): AsyncGenerator<RedisValue> {
    let cursor = "0";
    do {
      const page = await this.sscan(key, cursor, options);
      cursor = page.cursor;
      for (const member of page.items) yield member;
    } while (cursor !== "0");
  }

  /** Every member of a sorted set with its score, a page at a time. */
  async *zscanIterator(key: string, options: ScanOptions = {}): AsyncGenerator<[RedisValue, number]> {
    let cursor = "0";
    do {
      const page = await this.zscan(key, cursor, options);
      cursor = page.cursor;
      for (const entry of page.items) yield entry;
    } while (cursor !== "0");
  }

  // -- hashes ---------------------------------------------------------------

  async hget(key: string, field: string): Promise<RedisValue | null> {
    return (await this.call(["HGET", key, field])) as RedisValue | null;
  }

  /** Sets one field, or many when given an object. Returns fields **added**. */
  async hset(key: string, field: string | Record<string, CommandArg>, value?: CommandArg): Promise<number> {
    const args: CommandArg[] = ["HSET", key];
    if (typeof field === "string") {
      args.push(field, value as CommandArg);
    } else {
      for (const [name, item] of Object.entries(field)) args.push(name, item);
    }
    return count(await this.call(args));
  }

  async hsetnx(key: string, field: string, value: CommandArg): Promise<boolean> {
    return truthy(await this.call(["HSETNX", key, field, value]));
  }

  async hmget(key: string, ...fields: string[]): Promise<(RedisValue | null)[]> {
    return (await this.call(["HMGET", key, ...fields])) as (RedisValue | null)[];
  }

  async hdel(key: string, ...fields: string[]): Promise<number> {
    return count(await this.call(["HDEL", key, ...fields]));
  }

  /**
   * The whole hash as an object.
   *
   * Over RESP3 the server sends a map and this is exactly that. Over RESP2 it
   * sends a flat array of alternating fields and values, which is re-paired
   * here — so the two protocols answer the same shape, which is the sort of
   * difference a client exists to absorb.
   */
  async hgetall(key: string): Promise<Record<string, RedisValue>> {
    const reply = await this.call(["HGETALL", key]);
    if (Array.isArray(reply)) return pairs(reply);
    return (reply ?? {}) as Record<string, RedisValue>;
  }

  async hexists(key: string, field: string): Promise<boolean> {
    return truthy(await this.call(["HEXISTS", key, field]));
  }

  async hincrBy(key: string, field: string, by: number | bigint): Promise<number | bigint> {
    return integer(await this.call(["HINCRBY", key, field, by]));
  }

  async hincrByFloat(key: string, field: string, by: number): Promise<number> {
    return Number(await this.call(["HINCRBYFLOAT", key, field, by]));
  }

  async hkeys(key: string): Promise<string[]> {
    return ((await this.call(["HKEYS", key])) as unknown[]).map(String);
  }

  async hvals(key: string): Promise<RedisValue[]> {
    return (await this.call(["HVALS", key])) as RedisValue[];
  }

  async hlen(key: string): Promise<number> {
    return count(await this.call(["HLEN", key]));
  }

  /**
   * Expires individual hash **fields** (Redis 7.4+).
   *
   * Answers one status per field: `1` set, `0` refused because the condition
   * failed, `2` the field was deleted because the TTL was in the past, `-2` no
   * such field. Reported as the numbers Redis uses rather than flattened to a
   * boolean, because four outcomes do not fit in one.
   */
  async hexpire(key: string, seconds: number, ...fields: string[]): Promise<number[]> {
    const reply = (await this.call([
      "HEXPIRE", key, seconds, "FIELDS", fields.length, ...fields,
    ])) as unknown[];
    return reply.map(count);
  }

  async hpexpire(key: string, milliseconds: number, ...fields: string[]): Promise<number[]> {
    const reply = (await this.call([
      "HPEXPIRE", key, milliseconds, "FIELDS", fields.length, ...fields,
    ])) as unknown[];
    return reply.map(count);
  }

  /** Seconds left per field; `-1` no expiry, `-2` no such field. */
  async httl(key: string, ...fields: string[]): Promise<number[]> {
    const reply = (await this.call(["HTTL", key, "FIELDS", fields.length, ...fields])) as unknown[];
    return reply.map(count);
  }

  async hpersist(key: string, ...fields: string[]): Promise<number[]> {
    const reply = (await this.call([
      "HPERSIST", key, "FIELDS", fields.length, ...fields,
    ])) as unknown[];
    return reply.map(count);
  }

  /** One page of `HSCAN`, with the field/value pairs already re-paired. */
  async hscan(
    key: string,
    cursor: string | number = 0,
    options: ScanOptions = {},
  ): Promise<ScanPage<[string, RedisValue]>> {
    const args: CommandArg[] = ["HSCAN", key, String(cursor)];
    if (options.match !== undefined) args.push("MATCH", options.match);
    if (options.count !== undefined) args.push("COUNT", options.count);
    return page(await this.call(args), (items) => Object.entries(pairs(items)));
  }

  // -- lists ----------------------------------------------------------------

  async lpush(key: string, ...values: CommandArg[]): Promise<number> {
    return count(await this.call(["LPUSH", key, ...values]));
  }

  async rpush(key: string, ...values: CommandArg[]): Promise<number> {
    return count(await this.call(["RPUSH", key, ...values]));
  }

  /**
   * Pops from the head. With `count`, pops that many and answers an **array**.
   *
   * The two shapes are Redis's, not this client's: `LPOP key` answers a value
   * and `LPOP key 1` answers a one-element array. Flattening them would make
   * `lpop(key, n)` unable to say "the list was empty" distinctly from "it had
   * one element".
   */
  async lpop(key: string, count?: number): Promise<RedisValue | RedisValue[] | null> {
    const args: CommandArg[] = count === undefined ? ["LPOP", key] : ["LPOP", key, count];
    return (await this.call(args)) as RedisValue | RedisValue[] | null;
  }

  /** Pops from the tail; same two shapes as {@link lpop}. */
  async rpop(key: string, count?: number): Promise<RedisValue | RedisValue[] | null> {
    const args: CommandArg[] = count === undefined ? ["RPOP", key] : ["RPOP", key, count];
    return (await this.call(args)) as RedisValue | RedisValue[] | null;
  }

  /** `stop` is **inclusive**, and `-1` is the last element. */
  async lrange(key: string, start: number, stop: number): Promise<RedisValue[]> {
    return (await this.call(["LRANGE", key, start, stop])) as RedisValue[];
  }

  async llen(key: string): Promise<number> {
    return count(await this.call(["LLEN", key]));
  }

  async lrem(key: string, count_: number, value: CommandArg): Promise<number> {
    return count(await this.call(["LREM", key, count_, value]));
  }

  async lset(key: string, index: number, value: CommandArg): Promise<string> {
    return String(await this.call(["LSET", key, index, value]));
  }

  async lindex(key: string, index: number): Promise<RedisValue | null> {
    return (await this.call(["LINDEX", key, index])) as RedisValue | null;
  }

  async ltrim(key: string, start: number, stop: number): Promise<string> {
    return String(await this.call(["LTRIM", key, start, stop]));
  }

  /** The index of the first matching element, or `null`. */
  async lpos(key: string, value: CommandArg, options: { rank?: number } = {}): Promise<number | null> {
    const args: CommandArg[] = ["LPOS", key, value];
    if (options.rank !== undefined) args.push("RANK", options.rank);
    const reply = await this.call(args);
    return reply === null ? null : count(reply);
  }

  async lmove(
    source: string,
    destination: string,
    from: "LEFT" | "RIGHT" = "LEFT",
    to: "LEFT" | "RIGHT" = "RIGHT",
  ): Promise<RedisValue | null> {
    return (await this.call(["LMOVE", source, destination, from, to])) as RedisValue | null;
  }

  // -- sets -----------------------------------------------------------------

  async sadd(key: string, ...members: CommandArg[]): Promise<number> {
    return count(await this.call(["SADD", key, ...members]));
  }

  async srem(key: string, ...members: CommandArg[]): Promise<number> {
    return count(await this.call(["SREM", key, ...members]));
  }

  async smembers(key: string): Promise<RedisValue[]> {
    return (await this.call(["SMEMBERS", key])) as RedisValue[];
  }

  async sismember(key: string, member: CommandArg): Promise<boolean> {
    return truthy(await this.call(["SISMEMBER", key, member]));
  }

  async smismember(key: string, ...members: CommandArg[]): Promise<boolean[]> {
    return ((await this.call(["SMISMEMBER", key, ...members])) as unknown[]).map(truthy);
  }

  async scard(key: string): Promise<number> {
    return count(await this.call(["SCARD", key]));
  }

  async spop(key: string): Promise<RedisValue | null> {
    return (await this.call(["SPOP", key])) as RedisValue | null;
  }

  async srandmember(key: string, count_?: number): Promise<RedisValue | RedisValue[] | null> {
    const args: CommandArg[] = count_ === undefined ? ["SRANDMEMBER", key] : ["SRANDMEMBER", key, count_];
    return (await this.call(args)) as RedisValue | RedisValue[] | null;
  }

  async smove(source: string, destination: string, member: CommandArg): Promise<boolean> {
    return truthy(await this.call(["SMOVE", source, destination, member]));
  }

  /** How many members the intersection has, without building it. */
  async sintercard(keys: string[], options: { limit?: number } = {}): Promise<number> {
    const args: CommandArg[] = ["SINTERCARD", keys.length, ...keys];
    if (options.limit !== undefined) args.push("LIMIT", options.limit);
    return count(await this.call(args));
  }

  async sunion(...keys: string[]): Promise<RedisValue[]> {
    return (await this.call(["SUNION", ...keys])) as RedisValue[];
  }

  async sinter(...keys: string[]): Promise<RedisValue[]> {
    return (await this.call(["SINTER", ...keys])) as RedisValue[];
  }

  async sdiff(...keys: string[]): Promise<RedisValue[]> {
    return (await this.call(["SDIFF", ...keys])) as RedisValue[];
  }

  async sscan(
    key: string,
    cursor: string | number = 0,
    options: ScanOptions = {},
  ): Promise<ScanPage<RedisValue>> {
    const args: CommandArg[] = ["SSCAN", key, String(cursor)];
    if (options.match !== undefined) args.push("MATCH", options.match);
    if (options.count !== undefined) args.push("COUNT", options.count);
    return page(await this.call(args), (items) => items as RedisValue[]);
  }

  // -- sorted sets ----------------------------------------------------------

  /** `ZADD`, taking `{ member: score }`. Returns members **added**. */
  async zadd(
    key: string,
    scores: Record<string, number>,
    options: { nx?: boolean; xx?: boolean; gt?: boolean; lt?: boolean; ch?: boolean } = {},
  ): Promise<number> {
    const args: CommandArg[] = ["ZADD", key];
    if (options.nx) args.push("NX");
    if (options.xx) args.push("XX");
    if (options.gt) args.push("GT");
    if (options.lt) args.push("LT");
    if (options.ch) args.push("CH");
    for (const [member, score] of Object.entries(scores)) args.push(score, member);
    return count(await this.call(args));
  }

  async zrem(key: string, ...members: CommandArg[]): Promise<number> {
    return count(await this.call(["ZREM", key, ...members]));
  }

  /** The score, as a `number`. `null` when the member is not in the set. */
  async zscore(key: string, member: CommandArg): Promise<number | null> {
    const value = await this.call(["ZSCORE", key, member]);
    return value === null ? null : Number(value);
  }

  /**
   * `ZRANGE`. With `withScores` the result is `[member, score]` pairs, which is
   * the shape callers want — Redis sends them interleaved in RESP2 and paired
   * in RESP3, and both arrive here the same way.
   */
  async zrange(
    key: string,
    start: number | string,
    stop: number | string,
    options: { rev?: boolean; byScore?: boolean; byLex?: boolean; withScores?: boolean } = {},
  ): Promise<RedisValue[] | [RedisValue, number][]> {
    const args: CommandArg[] = ["ZRANGE", key, start, stop];
    if (options.byScore) args.push("BYSCORE");
    if (options.byLex) args.push("BYLEX");
    if (options.rev) args.push("REV");
    if (options.withScores) args.push("WITHSCORES");
    const reply = (await this.call(args)) as unknown[];
    return options.withScores ? scored(reply) : (reply as RedisValue[]);
  }

  async zcard(key: string): Promise<number> {
    return count(await this.call(["ZCARD", key]));
  }

  async zcount(key: string, min: number | string, max: number | string): Promise<number> {
    return count(await this.call(["ZCOUNT", key, min, max]));
  }

  async zincrBy(key: string, by: number, member: CommandArg): Promise<number> {
    return Number(await this.call(["ZINCRBY", key, by, member]));
  }

  async zrank(key: string, member: CommandArg): Promise<number | null> {
    const value = await this.call(["ZRANK", key, member]);
    return value === null ? null : count(value);
  }

  async zrevrank(key: string, member: CommandArg): Promise<number | null> {
    const value = await this.call(["ZREVRANK", key, member]);
    return value === null ? null : count(value);
  }

  /** The scores of several members at once; `null` for ones not in the set. */
  async zmscore(key: string, ...members: CommandArg[]): Promise<(number | null)[]> {
    const reply = (await this.call(["ZMSCORE", key, ...members])) as unknown[];
    return reply.map((score) => (score === null ? null : Number(score)));
  }

  /** Stores a range into another key, answering how many members it holds. */
  async zrangestore(
    destination: string,
    source: string,
    start: number | string,
    stop: number | string,
    options: { byScore?: boolean; byLex?: boolean; rev?: boolean } = {},
  ): Promise<number> {
    const args: CommandArg[] = ["ZRANGESTORE", destination, source, start, stop];
    if (options.byScore) args.push("BYSCORE");
    if (options.byLex) args.push("BYLEX");
    if (options.rev) args.push("REV");
    return count(await this.call(args));
  }

  async zscan(
    key: string,
    cursor: string | number = 0,
    options: ScanOptions = {},
  ): Promise<ScanPage<[RedisValue, number]>> {
    const args: CommandArg[] = ["ZSCAN", key, String(cursor)];
    if (options.match !== undefined) args.push("MATCH", options.match);
    if (options.count !== undefined) args.push("COUNT", options.count);
    return page(await this.call(args), scored);
  }

  // -- string ranges and bits -----------------------------------------------

  /** Overwrites part of a string, padding with NUL bytes if it has to. */
  async setrange(key: string, offset: number, value: CommandArg): Promise<number> {
    return count(await this.call(["SETRANGE", key, offset, value]));
  }

  /** A substring by byte offsets. `end` is **inclusive**, and `-1` is the last byte. */
  async getrange(key: string, start: number, end: number): Promise<RedisValue> {
    return (await this.call(["GETRANGE", key, start, end])) as RedisValue;
  }

  async setbit(key: string, offset: number, value: 0 | 1): Promise<number> {
    return count(await this.call(["SETBIT", key, offset, value]));
  }

  async getbit(key: string, offset: number): Promise<number> {
    return count(await this.call(["GETBIT", key, offset]));
  }

  /** How many bits are set. The range, if given, is in **bytes** unless `bit`. */
  async bitcount(
    key: string,
    range?: { start: number; end: number; bit?: boolean },
  ): Promise<number> {
    const args: CommandArg[] = ["BITCOUNT", key];
    if (range !== undefined) {
      args.push(range.start, range.end, range.bit ? "BIT" : "BYTE");
    }
    return count(await this.call(args));
  }

  /** The first bit set to `bit`, or `-1`. */
  async bitpos(key: string, bit: 0 | 1, range?: { start: number; end?: number }): Promise<number> {
    const args: CommandArg[] = ["BITPOS", key, bit];
    if (range !== undefined) {
      args.push(range.start);
      if (range.end !== undefined) args.push(range.end);
    }
    return count(await this.call(args));
  }

  /** A bitwise operation across keys, into `destination`. */
  async bitop(
    operation: "AND" | "OR" | "XOR" | "NOT",
    destination: string,
    ...keys: string[]
  ): Promise<number> {
    return count(await this.call(["BITOP", operation, destination, ...keys]));
  }

  // -- HyperLogLog ----------------------------------------------------------

  /**
   * Adds to a HyperLogLog — a set that counts its members in 12 KB however many
   * there are, at the cost of being approximate (about 0.81% error).
   *
   * Returns `true` when the estimate changed, which is not the same as "this
   * member was new".
   */
  async pfadd(key: string, ...members: CommandArg[]): Promise<boolean> {
    return truthy(await this.call(["PFADD", key, ...members]));
  }

  /** The approximate cardinality, across several keys if given. */
  async pfcount(...keys: string[]): Promise<number> {
    return count(await this.call(["PFCOUNT", ...keys]));
  }

  async pfmerge(destination: string, ...sources: string[]): Promise<string> {
    return String(await this.call(["PFMERGE", destination, ...sources]));
  }

  // -- streams --------------------------------------------------------------

  /**
   * Appends to a stream, answering the id it was given.
   *
   * `id` defaults to `*`, which asks the server to assign one — the ordinary
   * case, and the only one that cannot produce an out-of-order stream.
   * `maxlen` with `approximate` (the default) is how a stream is kept bounded
   * cheaply: Redis trims whole nodes rather than exact counts.
   */
  async xadd(
    key: string,
    fields: Record<string, CommandArg>,
    options: { id?: string; maxlen?: number; minid?: string; approximate?: boolean } = {},
  ): Promise<string> {
    const args: CommandArg[] = ["XADD", key];
    const approx = options.approximate !== false;
    if (options.maxlen !== undefined) args.push("MAXLEN", approx ? "~" : "=", options.maxlen);
    if (options.minid !== undefined) args.push("MINID", approx ? "~" : "=", options.minid);
    args.push(options.id ?? "*");
    for (const [field, value] of Object.entries(fields)) args.push(field, value);
    return String(await this.call(args));
  }

  async xlen(key: string): Promise<number> {
    return count(await this.call(["XLEN", key]));
  }

  /** Entries between two ids. `-` and `+` are the smallest and largest. */
  async xrange(
    key: string,
    start = "-",
    end = "+",
    options: { count?: number } = {},
  ): Promise<StreamEntry[]> {
    const args: CommandArg[] = ["XRANGE", key, start, end];
    if (options.count !== undefined) args.push("COUNT", options.count);
    return entries(await this.call(args));
  }

  async xrevrange(
    key: string,
    end = "+",
    start = "-",
    options: { count?: number } = {},
  ): Promise<StreamEntry[]> {
    const args: CommandArg[] = ["XREVRANGE", key, end, start];
    if (options.count !== undefined) args.push("COUNT", options.count);
    return entries(await this.call(args));
  }

  async xdel(key: string, ...ids: string[]): Promise<number> {
    return count(await this.call(["XDEL", key, ...ids]));
  }

  async xtrim(
    key: string,
    options: { maxlen?: number; minid?: string; approximate?: boolean },
  ): Promise<number> {
    const args: CommandArg[] = ["XTRIM", key];
    const approx = options.approximate !== false;
    if (options.maxlen !== undefined) args.push("MAXLEN", approx ? "~" : "=", options.maxlen);
    if (options.minid !== undefined) args.push("MINID", approx ? "~" : "=", options.minid);
    return count(await this.call(args));
  }

  /**
   * Reads from one or more streams, from an id exclusive.
   *
   * `streams` maps a key to the id to read *after* — `"0"` for everything so
   * far, `"$"` for only what arrives next. `block` waits that many
   * milliseconds; `0` would wait forever and is refused, for the reason every
   * blocking command is (see the README).
   */
  async xread(
    streams: Record<string, string>,
    options: { count?: number; block?: number } = {},
  ): Promise<Record<string, StreamEntry[]>> {
    const args: CommandArg[] = ["XREAD"];
    if (options.count !== undefined) args.push("COUNT", options.count);
    if (options.block !== undefined) args.push("BLOCK", options.block);
    const keys = Object.keys(streams);
    args.push("STREAMS", ...keys, ...keys.map((key) => streams[key]!));
    return streamReply(await this.call(args));
  }

  /** Creates a consumer group. `id` is where it starts — `$` for new entries only. */
  async xgroupCreate(
    key: string,
    group: string,
    id = "$",
    options: { mkstream?: boolean } = {},
  ): Promise<string> {
    const args: CommandArg[] = ["XGROUP", "CREATE", key, group, id];
    if (options.mkstream) args.push("MKSTREAM");
    return String(await this.call(args));
  }

  async xgroupDestroy(key: string, group: string): Promise<boolean> {
    return truthy(await this.call(["XGROUP", "DESTROY", key, group]));
  }

  /**
   * Reads as a member of a consumer group.
   *
   * `">"` means entries no one in the group has taken yet; any other id means
   * this consumer's own pending entries, for recovering after a crash.
   */
  async xreadgroup(
    group: string,
    consumer: string,
    streams: Record<string, string>,
    options: { count?: number; block?: number; noack?: boolean } = {},
  ): Promise<Record<string, StreamEntry[]>> {
    const args: CommandArg[] = ["XREADGROUP", "GROUP", group, consumer];
    if (options.count !== undefined) args.push("COUNT", options.count);
    if (options.block !== undefined) args.push("BLOCK", options.block);
    if (options.noack) args.push("NOACK");
    const keys = Object.keys(streams);
    args.push("STREAMS", ...keys, ...keys.map((key) => streams[key]!));
    return streamReply(await this.call(args));
  }

  /** Marks entries as handled, so they leave the group's pending list. */
  async xack(key: string, group: string, ...ids: string[]): Promise<number> {
    return count(await this.call(["XACK", key, group, ...ids]));
  }

  // -- geo ------------------------------------------------------------------

  /** Adds points, as `{ member: [longitude, latitude] }`. */
  async geoadd(key: string, members: Record<string, [number, number]>): Promise<number> {
    const args: CommandArg[] = ["GEOADD", key];
    for (const [member, [longitude, latitude]] of Object.entries(members)) {
      args.push(longitude, latitude, member);
    }
    return count(await this.call(args));
  }

  /** Where each member is, or `null` for one that is not there. */
  async geopos(key: string, ...members: string[]): Promise<(GeoPosition | null)[]> {
    const reply = (await this.call(["GEOPOS", key, ...members])) as unknown[];
    return reply.map((point) =>
      Array.isArray(point)
        ? { longitude: Number(point[0]), latitude: Number(point[1]) }
        : null,
    );
  }

  /** The distance between two members, or `null` if either is missing. */
  async geodist(
    key: string,
    from: string,
    to: string,
    unit: "m" | "km" | "mi" | "ft" = "m",
  ): Promise<number | null> {
    const value = await this.call(["GEODIST", key, from, to, unit]);
    return value === null ? null : Number(value);
  }

  /** Members within a radius of a member or a point. */
  async geosearch(
    key: string,
    options: {
      fromMember?: string;
      fromLonLat?: [number, number];
      byRadius?: number;
      byBox?: [number, number];
      unit?: "m" | "km" | "mi" | "ft";
      sort?: "ASC" | "DESC";
      count?: number;
    },
  ): Promise<RedisValue[]> {
    const args: CommandArg[] = ["GEOSEARCH", key];
    if (options.fromMember !== undefined) args.push("FROMMEMBER", options.fromMember);
    if (options.fromLonLat !== undefined) args.push("FROMLONLAT", ...options.fromLonLat);
    const unit = options.unit ?? "m";
    if (options.byRadius !== undefined) args.push("BYRADIUS", options.byRadius, unit);
    if (options.byBox !== undefined) args.push("BYBOX", ...options.byBox, unit);
    if (options.sort !== undefined) args.push(options.sort);
    if (options.count !== undefined) args.push("COUNT", options.count);
    return (await this.call(args)) as RedisValue[];
  }

  // -- scripting ------------------------------------------------------------

  /**
   * `EVAL`. `keys` is separate from `args` because Redis needs to know which
   * arguments are keys — it is how a cluster routes the script, and it is not
   * something a client can infer.
   */
  async eval(script: string, keys: string[] = [], args: CommandArg[] = []): Promise<unknown> {
    return this.call(["EVAL", script, keys.length, ...keys, ...args]);
  }

  async evalsha(sha: string, keys: string[] = [], args: CommandArg[] = []): Promise<unknown> {
    return this.call(["EVALSHA", sha, keys.length, ...keys, ...args]);
  }

  async scriptLoad(script: string): Promise<string> {
    return String(await this.call(["SCRIPT", "LOAD", script]));
  }

  // -- server and connection ------------------------------------------------

  // -- blocking -------------------------------------------------------------
  //
  // Every one of these takes its timeout as a **required** argument, in the
  // units Redis takes it: seconds for the pop family (fractions allowed since
  // 6.0), milliseconds for `wait`. Required rather than defaulted, because the
  // one value that must be a deliberate choice is the one that means "forever" —
  // and a default would make it an accident.
  //
  // A blocking command holds its connection for as long as it blocks. `0` is
  // allowed only on a connection opened with `{ blocking: true }`; see the
  // README.

  /**
   * `BLPOP` — pop from the head of the first list that has anything, waiting up
   * to `timeout` seconds. `null` when the wait expired.
   *
   * The reply is `[key, value]`, which is turned into an object here because
   * remembering which end of a two-element array is which is not a thing an API
   * should ask of anyone.
   */
  async blpop(keys: string | string[], timeout: number): Promise<{ key: string; value: RedisValue } | null> {
    return popped(await this.call(["BLPOP", ...many(keys), timeout]));
  }

  /** `BRPOP` — the same, from the tail. */
  async brpop(keys: string | string[], timeout: number): Promise<{ key: string; value: RedisValue } | null> {
    return popped(await this.call(["BRPOP", ...many(keys), timeout]));
  }

  /** `BLMOVE` — pop from one list and push onto another, atomically. */
  async blmove(
    source: string,
    destination: string,
    timeout: number,
    from: "LEFT" | "RIGHT" = "LEFT",
    to: "LEFT" | "RIGHT" = "RIGHT",
  ): Promise<RedisValue | null> {
    return (await this.call(["BLMOVE", source, destination, from, to, timeout])) as RedisValue | null;
  }

  /** `BZPOPMIN` — pop the lowest-scored member of the first non-empty set. */
  async bzpopmin(
    keys: string | string[],
    timeout: number,
  ): Promise<{ key: string; member: RedisValue; score: number } | null> {
    return zpopped(await this.call(["BZPOPMIN", ...many(keys), timeout]));
  }

  /** `BZPOPMAX` — the same, highest-scored. */
  async bzpopmax(
    keys: string | string[],
    timeout: number,
  ): Promise<{ key: string; member: RedisValue; score: number } | null> {
    return zpopped(await this.call(["BZPOPMAX", ...many(keys), timeout]));
  }

  /**
   * `WAIT` — block until `replicas` replicas have acknowledged the writes made
   * on this connection, or `timeoutMs` passes. Answers how many did.
   *
   * The timeout is in **milliseconds** here, unlike the pop family's seconds,
   * because that is what Redis takes. Note that a smaller answer than asked for
   * is not an error: it is the point of the return value.
   */
  async wait(replicas: number, timeoutMs: number): Promise<number> {
    return count(await this.call(["WAIT", replicas, timeoutMs]));
  }

  /**
   * Consumes a list as a queue, forever.
   *
   * The loop a blocking pop exists for, written once:
   *
   * ```js
   * const worker = await Redis.connect(url, { blocking: true });
   * for await (const job of worker.consume("jobs")) await handle(job.value);
   * ```
   *
   * It polls with a **bounded** `BLPOP` rather than an unbounded one even on a
   * blocking connection, because a bounded wait is what makes the loop
   * interruptible: an abandoned `for await` or an aborted signal is noticed
   * when the current wait ends rather than never. `timeout` is therefore the
   * worst case for how long stopping takes, not a latency — a job that arrives
   * mid-wait is delivered immediately.
   */
  async *consume(
    keys: string | string[],
    options: { timeout?: number; from?: "LEFT" | "RIGHT"; signal?: AbortSignal } = {},
  ): AsyncGenerator<{ key: string; value: RedisValue }> {
    const timeout = options.timeout ?? 5;
    const pop = options.from === "RIGHT" ? "BRPOP" : "BLPOP";
    const list = many(keys);
    for (;;) {
      if (options.signal?.aborted) return;
      const job = popped(await this.call([pop, ...list, timeout], { ...(options.signal ? { signal: options.signal } : {}) }));
      // `null` is the wait expiring with nothing to show, which is ordinary and
      // not the end of the queue — go round again.
      if (job !== null) yield job;
    }
  }

  // -- publishing -----------------------------------------------------------

  /**
   * Publishes to a channel, answering how many subscribers received it.
   *
   * An ordinary command on an ordinary connection — only the *subscribing* half
   * of pub/sub needs a connection of its own. A count of `0` means nobody was
   * listening, which Redis does not treat as an error and neither does this:
   * pub/sub is fire-and-forget, with no queue and no delivery guarantee.
   */
  async publish(channel: string, message: CommandArg): Promise<number> {
    return count(await this.call(["PUBLISH", channel, message]));
  }

  /** Publishes to a sharded channel (Redis 7+). */
  async spublish(channel: string, message: CommandArg): Promise<number> {
    return count(await this.call(["SPUBLISH", channel, message]));
  }

  /** The channels with at least one subscriber. */
  async pubsubChannels(pattern?: string): Promise<string[]> {
    const args: CommandArg[] = pattern === undefined ? ["PUBSUB", "CHANNELS"] : ["PUBSUB", "CHANNELS", pattern];
    return ((await this.call(args)) as unknown[]).map(String);
  }

  /** How many subscribers each named channel has. */
  async pubsubNumsub(...channels: string[]): Promise<Record<string, number>> {
    const reply = (await this.call(["PUBSUB", "NUMSUB", ...channels])) as unknown[];
    const out: Record<string, number> = {};
    for (let i = 0; i + 1 < reply.length; i += 2) out[String(reply[i])] = Number(reply[i + 1]);
    return out;
  }

  async ping(message?: string): Promise<string> {
    return String(await this.call(message === undefined ? ["PING"] : ["PING", message]));
  }

  async echo(message: string): Promise<RedisValue> {
    return (await this.call(["ECHO", message])) as RedisValue;
  }

  async dbsize(): Promise<number> {
    return count(await this.call(["DBSIZE"]));
  }

  async flushdb(options: { async?: boolean } = {}): Promise<string> {
    return String(await this.call(options.async ? ["FLUSHDB", "ASYNC"] : ["FLUSHDB"]));
  }

  async flushall(options: { async?: boolean } = {}): Promise<string> {
    return String(await this.call(options.async ? ["FLUSHALL", "ASYNC"] : ["FLUSHALL"]));
  }

  /** The raw `INFO` text — sections separated by `#` headings. */
  async info(section?: string): Promise<string> {
    return String(await this.call(section === undefined ? ["INFO"] : ["INFO", section]));
  }

  async configGet(pattern: string): Promise<Record<string, RedisValue>> {
    const reply = await this.call(["CONFIG", "GET", pattern]);
    if (Array.isArray(reply)) return pairs(reply);
    return (reply ?? {}) as Record<string, RedisValue>;
  }

  async configSet(parameter: string, value: CommandArg): Promise<string> {
    return String(await this.call(["CONFIG", "SET", parameter, value]));
  }

  // -- optimistic locking ---------------------------------------------------

  /**
   * `WATCH` — abort the next `EXEC` if any of these keys changes first.
   *
   * **It must be on the same connection as the `EXEC`**, which is what the
   * server ties it to. On a client that is automatic; on a pool it is not, so
   * use `withConnection()` and watch, read and exec inside it.
   */
  async watch(...keys: string[]): Promise<string> {
    return String(await this.call(["WATCH", ...keys]));
  }

  /** Forgets every `WATCH` on this connection. */
  async unwatch(): Promise<string> {
    return String(await this.call(["UNWATCH"]));
  }

  /** `[seconds, microseconds]`, as the server sees the clock. */
  async time(): Promise<[number, number]> {
    const reply = (await this.call(["TIME"])) as unknown[];
    return [Number(reply[0]), Number(reply[1])];
  }
}

// ---------------------------------------------------------------------------
// Narrowing what `call` hands back
// ---------------------------------------------------------------------------

/**
 * A reply that is a count, as a `number`.
 *
 * Distinct from {@link integer} on purpose: a cardinality, a length or a number
 * of keys removed cannot exceed what a `number` holds, so widening those to
 * `bigint` would push a union onto every caller for a case that cannot happen.
 * A *counter's value* is a different thing and keeps its `bigint`.
 */
function count(value: unknown): number {
  return Number(value);
}

function integer(value: unknown): number | bigint {
  return typeof value === "bigint" ? value : Number(value);
}

/** Redis says yes with `1` in RESP2 and `true` in RESP3. Both mean yes. */
function truthy(value: unknown): boolean {
  return value === true || value === 1 || value === 1n;
}

/** A flat `[field, value, field, value]` array as an object. */
function pairs(items: readonly unknown[]): Record<string, RedisValue> {
  const out: Record<string, RedisValue> = {};
  for (let i = 0; i + 1 < items.length; i += 2) {
    out[String(items[i])] = items[i + 1] as RedisValue;
  }
  return out;
}

/** `[member, score, …]` or `[[member, score], …]` as pairs with numeric scores. */
function scored(items: readonly unknown[]): [RedisValue, number][] {
  const out: [RedisValue, number][] = [];
  // RESP3 sends the pairs already grouped; RESP2 interleaves them.
  if (items.length > 0 && Array.isArray(items[0])) {
    for (const item of items as unknown[][]) out.push([item[0] as RedisValue, Number(item[1])]);
    return out;
  }
  for (let i = 0; i + 1 < items.length; i += 2) {
    out.push([items[i] as RedisValue, Number(items[i + 1])]);
  }
  return out;
}

/** One key, or several, as the argument list a command wants. */
function many(keys: string | string[]): string[] {
  return typeof keys === "string" ? [keys] : keys;
}

/** `BLPOP`'s `[key, value]`, or `null` when the wait expired. */
function popped(reply: unknown): { key: string; value: RedisValue } | null {
  if (reply === null || !Array.isArray(reply)) return null;
  return { key: String(reply[0]), value: reply[1] as RedisValue };
}

/** `BZPOPMIN`'s `[key, member, score]`. */
function zpopped(reply: unknown): { key: string; member: RedisValue; score: number } | null {
  if (reply === null || !Array.isArray(reply)) return null;
  return { key: String(reply[0]), member: reply[1] as RedisValue, score: Number(reply[2]) };
}

/** `[[id, [field, value, …]], …]` as entries. */
function entries(reply: unknown): StreamEntry[] {
  if (!Array.isArray(reply)) return [];
  return reply.map((entry) => {
    const pair = entry as unknown[];
    return { id: String(pair[0]), fields: pairs((pair[1] as unknown[]) ?? []) };
  });
}

/**
 * An `XREAD` reply, keyed by stream.
 *
 * RESP3 sends a map and RESP2 an array of `[name, entries]` pairs, so the two
 * protocols disagree about the shape of the same answer — which is exactly the
 * sort of difference a client exists to absorb. A read that timed out answers
 * null, which is no streams rather than an error.
 */
function streamReply(reply: unknown): Record<string, StreamEntry[]> {
  const out: Record<string, StreamEntry[]> = {};
  if (reply === null || reply === undefined) return out;
  if (Array.isArray(reply)) {
    for (const stream of reply) {
      const pair = stream as unknown[];
      out[String(pair[0])] = entries(pair[1]);
    }
    return out;
  }
  for (const [name, value] of Object.entries(reply as Record<string, unknown>)) {
    out[name] = entries(value);
  }
  return out;
}

/** A `SCAN`-family reply: `[cursor, items]`. */
function page<T>(reply: unknown, map: (items: unknown[]) => T[]): ScanPage<T> {
  const [cursor, items] = reply as [unknown, unknown[]];
  return { cursor: String(cursor), items: map(items ?? []) };
}

// The batch constructors, resolved on first use.
//
/**
 * Puts the command surface on a class that already has a base class.
 *
 * `RedisConnection` extends `runtime:db`'s `BaseConnection` and `RedisPooled`
 * extends its `PooledConnection`, so neither can also extend `RedisCommands` —
 * single inheritance, and both of those bases are the right one to have. The
 * methods are copied onto the prototype instead, which is what a mixin is when
 * the base is fixed rather than chosen.
 *
 * A method the target defines itself is **kept**: `call`, `execTransaction` and
 * `execPipeline` are the three each implementation supplies, and they are the
 * ones everything here is built from. Nothing is copied over them.
 *
 * The type half is a declaration merge at each call site
 * (`export interface RedisConnection extends RedisCommands {}`), so the
 * compiler sees the same surface the prototype gets.
 */
export function mixinCommands(target: { prototype: object }): void {
  const descriptors = Object.getOwnPropertyDescriptors(RedisCommands.prototype);
  for (const [name, descriptor] of Object.entries(descriptors)) {
    if (name === "constructor") continue;
    if (Object.hasOwn(target.prototype, name)) continue;
    Object.defineProperty(target.prototype, name, descriptor);
  }
}

// `batch.ts` imports this module to extend `RedisCommands`, so a static import
// back would be a cycle: whichever module the loader reached second would see
// the other's binding still uninitialized, and `multi()` would fail with a TDZ
// error rather than anything that names the problem.
interface Batches {
  transaction: new (runner: TransactionRunner) => RedisTransaction;
  pipeline: new (runner: TransactionRunner) => RedisPipeline;
}

let registered: Batches | null = null;

/** Registered by `batch.ts`'s module, breaking the cycle at run time. */
export function registerBatches(
  transaction: Batches["transaction"],
  pipeline: Batches["pipeline"],
): void {
  registered = { transaction, pipeline };
}

function batches(): Batches {
  if (registered === null) {
    throw new Error("the batch module was not loaded — import the package entry point");
  }
  return registered;
}
