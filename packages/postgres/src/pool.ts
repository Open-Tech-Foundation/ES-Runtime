/**
 * A pool of PostgreSQL connections, presenting the same surface one connection
 * does.
 *
 * Almost nothing is left here, and that is the point. Borrowing per call,
 * returning a connection when a streaming result ends, refusing to reuse one
 * that came back dirty, the pool's own counters — all of that is
 * `PooledConnection` in `runtime:db`, and every driver gets it identically.
 * What remains is the part that is PostgreSQL's: `executeScript`, and the
 * types that say a borrowed connection is a `PgConnection`.
 */
import {
  PooledConnection,
  type AnyDriver,
  type CallOptions,
  type Connection,
  type DbParams,
  type PoolSettings,
  type Queryable,
  type Rows,
} from "runtime:db";

import { PgConnection, type PgOptions, type PgRow } from "./connection.js";

/** Connection options, plus how big the pool is. */
export interface PgPoolOptions extends PgOptions, PoolSettings {}

export class PgPooled extends PooledConnection {
  constructor(
    driver: AnyDriver,
    url: string,
    options: PgOptions = {},
    poolOptions: PoolSettings = {},
  ) {
    super(driver, url, options, poolOptions);
  }

  /**
   * Runs `fn` with one connection held for the whole of it.
   *
   * Typed to `PgConnection` rather than `Connection`, because the reason to
   * reach for it is usually something only this driver has — a `LISTEN`, a
   * session setting, a `COPY`.
   */
  override withConnection<T>(fn: (connection: PgConnection) => Promise<T>): Promise<T> {
    return super.withConnection(fn as (connection: Connection) => Promise<T>);
  }

  override transaction<T>(fn: (tx: PgConnection) => Promise<T>): Promise<T> {
    return super.transaction(fn as (tx: Connection) => Promise<T>);
  }

  /** Rows from this backend, typed as this backend decodes them. */
  override query(
    q: Queryable,
    params?: DbParams,
    options?: CallOptions,
  ): Promise<Rows<PgRow>> {
    return super.query(q, params, options) as Promise<Rows<PgRow>>;
  }

  /** Runs a script on a borrowed connection. */
  executeScript(
    sql: string,
    options: { signal?: AbortSignal } = {},
  ): Promise<{ command: string; changes: number }[]> {
    return this.withConnection((connection) => connection.executeScript(sql, options));
  }
}
