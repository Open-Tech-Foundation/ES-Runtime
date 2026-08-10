/**
 * A pooled PostgreSQL connection.
 *
 * Presents the same surface as a single connection — `query`, `execute`,
 * `executeMany`, `transaction` — and borrows a real one per operation. The
 * generic half lives in `runtime:db`'s `Pool`; what is here is the part that
 * needs the protocol: deciding whether a connection coming back is fit for the
 * next caller.
 */
import { Pool, type Connection, type ExecuteResult, type Rows } from "runtime:db";

import { PgConnection, type PgOptions } from "./connection.js";

export interface PgPoolOptions extends PgOptions {
  /** Connections to open at most. Default 10. */
  max?: number;
  /** How long an unused connection is kept, in ms. Default 30 000. */
  idleTimeout?: number;
  /** How long to wait for a free connection, in ms. Default 10 000. */
  acquireTimeout?: number;
}

export class PgPool {
  readonly #pool: Pool<PgConnection>;

  constructor(open: () => Promise<PgConnection>, options: PgPoolOptions = {}) {
    this.#pool = new Pool<PgConnection>({
      create: open,
      destroy: (connection: PgConnection) => connection.close(),
      // Checked on the way out as well as asserted on the way in: a connection
      // can die while nobody is holding it — a server restart, an idle timeout
      // at the far end — and the first a pool hears of that is when it hands
      // the corpse to someone.
      validate: (connection: PgConnection) => connection.usable,
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
   * Returns a connection to the pool, saying whether it is reusable.
   *
   * `I` is PostgreSQL's own answer: the last `ReadyForQuery` said this session
   * is idle, outside any transaction. `T` means a transaction is still open and
   * `E` means one failed and has not been rolled back — either would leak into
   * whoever borrowed it next, so both are destroyed instead. That is the whole
   * of `release(clean)`, and the reason the pool cannot decide it alone.
   */
  #give(connection: PgConnection): void {
    this.#pool.release(connection, { clean: connection.usable && connection.status === "I" });
  }

  async query(q: Parameters<Connection["query"]>[0], params?: Parameters<Connection["query"]>[1]): Promise<Rows> {
    const connection = await this.#pool.acquire();
    try {
      const rows = await connection.query(q, params);
      if (rows.exhausted) {
        // The whole result already arrived, so the connection is free before
        // the caller reads a row — which is most queries, and the case where
        // holding it until an iterator happened to finish would waste it.
        this.#give(connection);
        return rows;
      }
      // A streaming result owns the connection until it ends. `Rows` closes
      // itself however the iteration finishes, so this rides on that rather
      // than asking the caller to remember.
      const close = rows.close.bind(rows);
      rows.close = async () => {
        try {
          await close();
        } finally {
          this.#give(connection);
        }
      };
      return rows;
    } catch (e) {
      this.#give(connection);
      throw e;
    }
  }

  async execute(
    q: Parameters<Connection["execute"]>[0],
    params?: Parameters<Connection["execute"]>[1],
  ): Promise<ExecuteResult> {
    const connection = await this.#pool.acquire();
    try {
      return await connection.execute(q, params);
    } finally {
      this.#give(connection);
    }
  }

  async executeMany(
    q: Parameters<Connection["executeMany"]>[0],
    rows: Parameters<Connection["executeMany"]>[1],
  ): Promise<ExecuteResult> {
    const connection = await this.#pool.acquire();
    try {
      return await connection.executeMany(q, rows);
    } finally {
      this.#give(connection);
    }
  }

  /**
   * Runs `fn` in a transaction on **one** connection.
   *
   * The connection is held for the whole of it, which is the point: a
   * transaction spread across connections is not a transaction. The `tx` handed
   * to `fn` is that connection, so everything inside runs on it.
   */
  async transaction<T>(fn: (tx: PgConnection) => Promise<T>): Promise<T> {
    const connection = await this.#pool.acquire();
    try {
      return await connection.transaction(fn as (tx: Connection) => Promise<T>);
    } finally {
      this.#give(connection);
    }
  }

  /** Runs a script on a borrowed connection. */
  async executeScript(sql: string): Promise<{ command: string; changes: number }[]> {
    const connection = await this.#pool.acquire();
    try {
      return await connection.executeScript(sql);
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
