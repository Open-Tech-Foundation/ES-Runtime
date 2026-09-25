/**
 * A pool of MySQL connections, presenting the same surface one connection
 * does.
 *
 * Borrowing per call, returning a connection when a streaming result ends,
 * refusing to reuse one that came back inside a transaction — all of that is
 * `PooledConnection` in `runtime:db`, and every driver gets it identically.
 * What remains is the part that is MySQL's: `executeScript`, and the types that
 * say a borrowed connection is a `MySqlConnection`.
 */
import {
  type AnyDriver,
  type CallOptions,
  type Connection,
  type DbParams,
  PooledConnection,
  type PoolSettings,
  type Queryable,
  type Rows,
} from "runtime:db";

import type { MySqlConnection, MySqlOptions, MySqlRow } from "./connection.js";

/** Connection options, plus how big the pool is. */
export interface MySqlPoolOptions extends MySqlOptions, PoolSettings {}

export class MySqlPooled extends PooledConnection {
  constructor(
    driver: AnyDriver,
    url: string,
    options: MySqlOptions = {},
    poolOptions: PoolSettings = {},
  ) {
    super(driver, url, options, poolOptions);
  }

  override withConnection<T>(fn: (connection: MySqlConnection) => Promise<T>): Promise<T> {
    return super.withConnection(fn as (connection: Connection) => Promise<T>);
  }

  override transaction<T>(fn: (tx: MySqlConnection) => Promise<T>): Promise<T> {
    return super.transaction(fn as (tx: Connection) => Promise<T>);
  }

  /** Rows from this backend, typed as this backend decodes them. */
  override query(q: Queryable, params?: DbParams, options?: CallOptions): Promise<Rows<MySqlRow>> {
    return super.query(q, params, options) as Promise<Rows<MySqlRow>>;
  }

  /** Runs a script on a borrowed connection. */
  executeScript(
    sql: string,
    options: { signal?: AbortSignal } = {},
  ): Promise<{ changes: number; lastInsertRowid: number | null }[]> {
    return this.withConnection((connection) => connection.executeScript(sql, options));
  }
}
