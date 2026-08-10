/**
 * `Redis` — the client most callers want.
 *
 * One connection, the full command surface from {@link RedisCommands}, and the
 * `runtime:db` shapes still underneath it for anything that wants them. The
 * split matters: `RedisConnection` is a `runtime:db` backend and answers
 * `query`/`execute` in that vocabulary, and this is the object you reach for
 * when you are writing Redis rather than writing something portable.
 */
import { queryAst, type ExecuteResult, type Rows } from "runtime:db";

import { RedisCommands } from "./commands.js";
import {
  RedisConnection,
  type MessageHandler,
  type RedisOptions,
  type ServerHello,
} from "./connection.js";
import type { CommandArg } from "./protocol/resp.js";
import { parseConnectionString } from "./url.js";

export class Redis extends RedisCommands {
  readonly connection: RedisConnection;

  constructor(connection: RedisConnection) {
    super();
    this.connection = connection;
  }

  /** Opens a client. `redis://localhost` is the whole of the usual case. */
  static async connect(url: string, options: RedisOptions = {}): Promise<Redis> {
    const connection = new RedisConnection();
    await connection.open(parseConnectionString(url, options));
    return new Redis(connection);
  }

  override call(args: readonly CommandArg[], options: { signal?: AbortSignal } = {}): Promise<unknown> {
    return this.connection.command(args, options);
  }

  /** Runs a transaction built by `multi()`, on this client's connection. */
  override execTransaction(commands: readonly (readonly CommandArg[])[]): Promise<unknown[] | null> {
    return this.connection.execTransaction(commands);
  }

  /** What the server said at `HELLO` — its version, id, role, and protocol. */
  get server(): Partial<ServerHello> {
    return this.connection.hello;
  }

  /** The protocol in force: 3, or 2 against a server without RESP3. */
  get protocol(): number {
    return this.connection.protocol;
  }

  /** Whether this connection is still worth using. */
  get usable(): boolean {
    return this.connection.usable;
  }

  // -- pub/sub ---------------------------------------------------------------
  //
  // Forwarded rather than reimplemented. Note what subscribing costs: the first
  // one gives this client's connection over to a read loop, and every command
  // method above then refuses with `ERR_DB_CONNECTION_BUSY`. Use
  // `createSubscriber()` for a client that only listens, and keep another for
  // work — which is what `publish` needs anyway, since you cannot publish from
  // a subscribed connection.

  subscribe(channels: string | readonly string[], handler?: MessageHandler): Promise<void> {
    return this.connection.subscribe(channels, handler);
  }

  psubscribe(patterns: string | readonly string[], handler?: MessageHandler): Promise<void> {
    return this.connection.psubscribe(patterns, handler);
  }

  ssubscribe(channels: string | readonly string[], handler?: MessageHandler): Promise<void> {
    return this.connection.ssubscribe(channels, handler);
  }

  unsubscribe(channels?: string | readonly string[]): Promise<void> {
    return this.connection.unsubscribe(channels);
  }

  punsubscribe(patterns?: string | readonly string[]): Promise<void> {
    return this.connection.punsubscribe(patterns);
  }

  sunsubscribe(channels?: string | readonly string[]): Promise<void> {
    return this.connection.sunsubscribe(channels);
  }

  get subscribed(): boolean {
    return this.connection.subscribed;
  }

  get channels(): string[] {
    return this.connection.channels;
  }

  get patterns(): string[] {
    return this.connection.patterns;
  }

  get shardChannels(): string[] {
    return this.connection.shardChannels;
  }

  /** The catch-all message handler; see `RedisConnection.onMessage`. */
  set onMessage(handler: MessageHandler | undefined) {
    this.connection.onMessage = handler;
  }

  get onMessage(): MessageHandler | undefined {
    return this.connection.onMessage;
  }

  set onSubscribeError(handler: ((error: unknown) => void) | undefined) {
    this.connection.onSubscribeError = handler;
  }

  get onSubscribeError(): ((error: unknown) => void) | undefined {
    return this.connection.onSubscribeError;
  }

  /**
   * The same command, read as rows.
   *
   * A convenience over the backend underneath rather than a second way of
   * talking to Redis: an aggregate reply becomes one row per element, a map
   * becomes `field`/`value` rows, and anything else becomes a single row.
   */
  query(command: readonly CommandArg[]): Promise<Rows> {
    return this.connection.query(queryAst(command));
  }

  execute(command: readonly CommandArg[]): Promise<ExecuteResult> {
    return this.connection.execute(queryAst(command));
  }

  async close(): Promise<void> {
    await this.connection.close();
  }

  async [Symbol.asyncDispose](): Promise<void> {
    await this.close();
  }
}
