/**
 * A pool of Redis connections, presenting exactly what one connection does.
 *
 * The generic half — borrowing per call, refusing to reuse a connection that
 * came back dirty, the counters, `withConnection` — is `PooledConnection` in
 * `runtime:db`, and every driver gets it identically. What is here is Redis's
 * own: the command surface, and the two batch forms that must land on a single
 * connection.
 */
import {
  PooledConnection,
  type AnyDriver,
  type Connection,
  type PoolSettings,
} from "runtime:db";

import { RedisCommands, mixinCommands } from "./commands.js";
import { RedisConnection, type RedisOptions } from "./connection.js";
import type { CommandArg } from "./protocol/resp.js";

/** Connection options, plus how big the pool is. */
export interface RedisPoolOptions extends RedisOptions, PoolSettings {}

export class RedisPooled extends PooledConnection {
  constructor(
    driver: AnyDriver,
    url: string,
    options: RedisOptions = {},
    poolOptions: PoolSettings = {},
  ) {
    super(driver, url, options, poolOptions);
  }

  /** Runs one command on a borrowed connection. Everything else is built on it. */
  call(args: readonly CommandArg[], options: { signal?: AbortSignal } = {}): Promise<unknown> {
    return this.withConnection((connection) => connection.call(args, options));
  }

  /**
   * Runs a transaction built by `multi()` on **one** borrowed connection.
   *
   * A pool can do this at all only because the commands were buffered: there is
   * nothing to hold a connection for until `exec()`, and then the whole
   * `MULTI`…`EXEC` goes out as one batch. `WATCH` is the exception — the server
   * ties it to a connection, so optimistic locking needs `withConnection()`.
   */
  execTransaction(commands: readonly (readonly CommandArg[])[]): Promise<unknown[] | null> {
    return this.withConnection((connection) => connection.execTransaction(commands));
  }

  /** Runs a pipeline on one borrowed connection, for the same reason. */
  execPipeline(commands: readonly (readonly CommandArg[])[]): Promise<unknown[]> {
    return this.withConnection((connection) => connection.execPipeline(commands));
  }

  /**
   * Runs `fn` with one connection held for the whole of it.
   *
   * Typed to `RedisConnection`, because what needs it is Redis's own state:
   * a `WATCH`, a `SELECT` and the commands that depend on it, a `MULTI` sent by
   * hand. Borrowing per command would spread those over connections that do not
   * share the state.
   */
  override withConnection<T>(fn: (connection: RedisConnection) => Promise<T>): Promise<T> {
    return super.withConnection(fn as unknown as (connection: Connection) => Promise<T>);
  }
}

/** The command surface, on the pooled form too — see `mixinCommands`. */
export interface RedisPooled extends RedisCommands {}
mixinCommands(RedisPooled);
