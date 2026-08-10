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

  async lpop(key: string): Promise<RedisValue | null> {
    return (await this.call(["LPOP", key])) as RedisValue | null;
  }

  async rpop(key: string): Promise<RedisValue | null> {
    return (await this.call(["RPOP", key])) as RedisValue | null;
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

/** A `SCAN`-family reply: `[cursor, items]`. */
function page<T>(reply: unknown, map: (items: unknown[]) => T[]): ScanPage<T> {
  const [cursor, items] = reply as [unknown, unknown[]];
  return { cursor: String(cursor), items: map(items ?? []) };
}
