/**
 * A pool of Redis connections, presenting the same commands one connection does.
 *
 * The generic half lives in `runtime:db`'s `Pool`. What is here is the part
 * that needs the protocol — deciding whether a connection coming back is fit
 * for the next caller — which is the one decision a protocol-blind pool cannot
 * make for itself.
 */
import { Pool, queryAst, type ExecuteResult, type Rows } from "runtime:db";

import { RedisCommands } from "./commands.js";
import { RedisConnection, type RedisOptions } from "./connection.js";
import type { CommandArg } from "./protocol/resp.js";

export interface RedisPoolOptions extends RedisOptions {
  /** Connections to open at most. Default 10. */
  max?: number;
  /** How long an unused connection is kept, in ms. Default 30 000. */
  idleTimeout?: number;
  /** How long to wait for a free connection, in ms. Default 10 000. */
  acquireTimeout?: number;
}

export class RedisPool extends RedisCommands {
  readonly #pool: Pool<RedisConnection>;

  constructor(open: () => Promise<RedisConnection>, options: RedisPoolOptions = {}) {
    super();
    this.#pool = new Pool<RedisConnection>({
      create: open,
      destroy: (connection) => connection.close(),
      // Checked on the way out as well as asserted on the way in: a connection
      // can die while nobody is holding it — a server restart, an idle timeout
      // at the far end — and the first a pool hears of that is otherwise when
      // it hands the corpse to someone.
      validate: (connection) => connection.usable,
      max: options.max ?? 10,
      idleTimeout: options.idleTimeout ?? 30_000,
      acquireTimeout: options.acquireTimeout ?? 10_000,
    });
  }

  get size(): number {
    return this.#pool.size;
  }

  get idle(): number {
    return this.#pool.idle;
  }

  get pending(): number {
    return this.#pool.pending;
  }

  /**
   * Returns a connection, saying whether it is reusable.
   *
   * `connection.clean` is the driver's own answer: alive, on the database it
   * was opened for, and not inside a `MULTI`. Anything else is destroyed —
   * `release(clean)` defaults to false precisely so that a connection nobody
   * vouched for does not become the next request's problem.
   */
  #give(connection: RedisConnection): void {
    this.#pool.release(connection, { clean: connection.clean });
  }

  override async call(
    args: readonly CommandArg[],
    options: { signal?: AbortSignal } = {},
  ): Promise<unknown> {
    const connection = await this.#pool.acquire();
    try {
      return await connection.command(args, options);
    } finally {
      this.#give(connection);
    }
  }

  /**
   * Runs a transaction built by `multi()` on **one** borrowed connection.
   *
   * A pool can do this at all only because the commands were buffered: there is
   * nothing to hold a connection for until `exec()`, and then the whole
   * `MULTI`…`EXEC` goes out as one batch. `WATCH` is the exception — the server
   * ties it to a connection, so optimistic locking needs `withConnection()`.
   */
  override async execTransaction(
    commands: readonly (readonly CommandArg[])[],
  ): Promise<unknown[] | null> {
    const connection = await this.#pool.acquire();
    try {
      return await connection.execTransaction(commands);
    } finally {
      this.#give(connection);
    }
  }

  /**
   * A command read as rows.
   *
   * The connection is returned before the caller reads a row, and that is
   * correct here where it would not be on a SQL backend: a RESP reply is
   * complete once it has been read, so `rows.exhausted` is always true and
   * there is no cursor holding the connection open.
   */
  async query(command: readonly CommandArg[]): Promise<Rows> {
    const connection = await this.#pool.acquire();
    try {
      return await connection.query(queryAst(command));
    } finally {
      this.#give(connection);
    }
  }

  async execute(command: readonly CommandArg[]): Promise<ExecuteResult> {
    const connection = await this.#pool.acquire();
    try {
      return await connection.execute(queryAst(command));
    } finally {
      this.#give(connection);
    }
  }

  /**
   * Runs `fn` with one connection held for the whole of it.
   *
   * The escape hatch for the few things that are stateful across commands —
   * a `MULTI`/`EXEC` sent by hand, a `SELECT` and the commands that depend on
   * it — where borrowing per command would spread them over connections that
   * do not share the state.
   */
  async withConnection<T>(fn: (connection: RedisConnection) => Promise<T>): Promise<T> {
    const connection = await this.#pool.acquire();
    try {
      return await fn(connection);
    } finally {
      this.#give(connection);
    }
  }

  async close(): Promise<void> {
    await this.#pool.close();
  }

  async [Symbol.asyncDispose](): Promise<void> {
    await this.close();
  }
}
