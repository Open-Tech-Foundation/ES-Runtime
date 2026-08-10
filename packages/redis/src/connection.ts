/**
 * A Redis connection: RESP, in JavaScript, over `runtime:net`.
 *
 * No native code, and no new runtime code either — which is the rule D56 set
 * for itself and named Redis in: "adding MySQL, Redis, or any other socket
 * backend must require zero new Rust." The transport is a socket and, for
 * `rediss://`, a TLS one; everything above it is here.
 *
 * What Redis is not, and the design follows from it: it is not a SQL database.
 * There is no statement text, no result description, no rows and no columns. So
 * this backend takes `queryAst([...])` — a command as an array — which is the
 * form `runtime:db` has carried since its first release precisely so that an
 * engine which never speaks SQL could be a first-class backend rather than a
 * special case. It is the first one to use it.
 */
import { connect as netConnect } from "runtime:net";
import {
  BaseConnection,
  DbError,
  DbErrorCode,
  Dialect,
  Rows,
  defineRowShape,
  type ExecuteResult,
  type NormalizedQuery,
} from "runtime:db";

import { RespReader, encodeCommand, type CommandArg, type Reply } from "./protocol/resp.js";
import { portableCode, redirectMessage } from "./protocol/errors.js";
import { shapeOf, toValue, writeRows, type DecodeOptions } from "./protocol/values.js";

/** What `runtime:net`'s `connect()` hands back. */
type RedisSocket = ReturnType<typeof netConnect>;

const BATCH_BYTES = 64 * 1024;

/**
 * Redis's dialect, which is mostly a list of things it is not.
 *
 * `sqlText: false` and `queryAst: true` are the two that matter: this backend
 * refuses SQL by name, and takes command arrays. Everything else follows from
 * Redis rather than from a choice made here — there are no placeholders because
 * there is no statement to put them in, and no savepoints because there is
 * nothing to nest.
 */
export const REDIS_DIALECT = new Dialect({
  name: "redis",
  // A query builder that reaches for this is building SQL for a backend that
  // has none. Failing loudly beats returning `$1` and letting it be
  // concatenated into a string nothing will ever parse.
  placeholder: () => {
    throw new DbError(
      "the redis backend has no SQL and no placeholders — pass a command array to queryAst()",
      { code: DbErrorCode.QueryForm },
    );
  },
  supports: {
    returning: false,
    savepoints: false,
    namedParameters: false,
    sqlText: false,
    queryAst: true,
    // MULTI/EXEC is not a transaction in the sense `transaction(fn)` promises.
    // It queues commands and applies them together, but a command that fails at
    // EXEC time does not roll back the ones beside it — so a `transaction()`
    // built on it would commit half a body that threw. Declared absent rather
    // than approximated; see the README.
    transactions: false,
  },
});

export interface RedisOptions extends DecodeOptions {
  host?: string;
  port?: number;
  /** The ACL username. Omitted means Redis's `default` user. */
  username?: string;
  password?: string;
  /** The database index to `SELECT` after connecting. Default 0. */
  db?: number;
  /** `CLIENT SETNAME`, which makes a connection identifiable in `CLIENT LIST`. */
  clientName?: string;
  /** TLS from the first byte, as `rediss://` implies. Redis has no STARTTLS. */
  tls?: boolean;
  /** A certificate authority to trust in addition to the public roots, as PEM. */
  tlsCa?: string | Uint8Array;
  /**
   * How long to wait for the connection **and its handshake**, in
   * milliseconds. Default 10 000; `0` waits forever.
   */
  connectTimeout?: number;
  /**
   * Ask for RESP3. Default `true`.
   *
   * RESP3 is worth the default because it types the reply: `HGETALL` comes back
   * as a map rather than a flat array that the client has to know to re-pair,
   * and a double is a double rather than a string. A server older than Redis 6
   * has no `HELLO` at all, and this falls back to RESP2 on its own — so the
   * option is for forcing RESP2 against a server that has both.
   */
  resp3?: boolean;
}

/** What the server said about itself in its `HELLO` reply. */
export interface ServerHello {
  server: string;
  version: string;
  proto: number;
  id: number;
  mode: string;
  role: string;
}

export class RedisConnection extends BaseConnection {
  #socket: RedisSocket | null = null;
  #replies: RespReader | null = null;
  #writer: WritableStreamDefaultWriter<Uint8Array> | null = null;
  #fatal: DbError | null = null;
  #decode: DecodeOptions = {};

  /** What the server reported at `HELLO`. Empty until the handshake finishes. */
  readonly hello: Partial<ServerHello> = {};

  /** The protocol actually in force — 3, or 2 after a fallback. */
  get protocol(): number {
    return this.hello.proto ?? 2;
  }

  /**
   * One exchange at a time.
   *
   * A connection is one conversation here as much as anywhere, but the answer
   * differs from the PostgreSQL driver's and the reason is worth stating.
   * There, a second query issued while a result set is streaming is **refused**,
   * because the open result only ends when the caller drains it and queueing
   * behind it would deadlock. Redis has no cursor: a reply is complete when it
   * has been read, and every exchange finishes on its own without the caller
   * doing anything. So queueing here is bounded, and the queue is the right
   * answer rather than a refusal.
   */
  #lock: Promise<unknown> = Promise.resolve();

  /** The database this connection is on, and the one it was asked for. */
  #db = 0;
  #wantedDb = 0;
  /** Set between `MULTI` and the `EXEC`/`DISCARD` that ends it. */
  #inMulti = false;

  constructor() {
    super({ dialect: REDIS_DIALECT, backend: "redis" });
  }

  /**
   * Whether this connection is still worth handing to anyone.
   *
   * A pool asks before offering one, because a connection can die while nobody
   * holds it — a server restart, an idle timeout at the far end — and the first
   * anyone hears of that is otherwise the next caller's error.
   */
  get usable(): boolean {
    // `_close()` nulls the socket, so a closed connection reports unusable
    // without this needing to reach into the base class's state.
    return this.#fatal === null && this.#socket !== null;
  }

  /**
   * Whether this connection is fit for the **next** caller.
   *
   * `Pool` cannot decide this: it needs the protocol. Two things make a Redis
   * connection unfit even though it is alive. It may be on a different database
   * than it was opened for, because something ran `SELECT` — handing that to
   * the next borrower would silently point their keys at another dataset. And
   * it may be inside a `MULTI`, holding a queue of commands nobody is going to
   * `EXEC`.
   */
  get clean(): boolean {
    return this.usable && this.#db === this.#wantedDb && !this.#inMulti;
  }

  // -- lifecycle ------------------------------------------------------------

  async open(options: RedisOptions): Promise<void> {
    const budget = options.connectTimeout ?? 10_000;
    if (budget <= 0) return this.#open(options);
    let timer: ReturnType<typeof setTimeout> | undefined;
    const expired = new Promise<never>((_, reject) => {
      timer = setTimeout(() => {
        reject(
          new DbError(
            `the connection to ${options.host ?? "localhost"}:${options.port ?? 6379} did not complete within ${budget}ms`,
            { code: DbErrorCode.Timeout },
          ),
        );
      }, budget);
    });
    try {
      await Promise.race([this.#open(options), expired]);
    } catch (e) {
      // A socket may be half-open behind a rejected race, and a descriptor left
      // dangling on a timeout is how a retry loop runs a process out of them.
      await this.#teardown().catch(() => {});
      throw e;
    } finally {
      clearTimeout(timer);
    }
  }

  async #open(options: RedisOptions): Promise<void> {
    this.#decode = options.binary === undefined ? {} : { binary: options.binary };
    const host = options.host ?? "localhost";
    const port = options.port ?? 6379;

    // TLS from the first byte. Redis has no in-band upgrade — there is no
    // `SSLRequest` equivalent — so `rediss://` is an ordinary TLS socket, which
    // is the one place this handshake is simpler than PostgreSQL's.
    const socket = netConnect(
      { hostname: host, port },
      options.tls
        ? { secureTransport: "on", ...(options.tlsCa === undefined ? {} : { ca: options.tlsCa }) }
        : {},
    );
    await socket.opened;
    this.#socket = socket;
    this.#replies = new RespReader(socket.readable);
    this.#writer = socket.writable.getWriter();

    await this.#handshake(options);

    const db = options.db ?? 0;
    this.#wantedDb = db;
    if (db !== 0) {
      await this.#exchange(["SELECT", db]);
      this.#db = db;
    }
    if (options.clientName !== undefined) {
      await this.#exchange(["CLIENT", "SETNAME", options.clientName]);
    }
  }

  /**
   * `HELLO`, which negotiates the protocol and authenticates in one round trip.
   *
   * Two ways it can fail that are not failures. A server older than Redis 6 has
   * no `HELLO` and answers `ERR unknown command`; a server that has it but was
   * built without RESP3 answers `NOPROTO`. Both mean "RESP2, then", and both
   * leave the authentication undone — so the fallback has to `AUTH` separately
   * rather than assume the failed `HELLO` did it. Anything else is a real
   * failure and is rethrown: a wrong password must not quietly become a
   * downgrade.
   */
  async #handshake(options: RedisOptions): Promise<void> {
    const auth: CommandArg[] =
      options.password === undefined
        ? []
        : ["AUTH", options.username ?? "default", options.password];

    if (options.resp3 !== false) {
      try {
        const reply = await this.#exchange(["HELLO", 3, ...auth]);
        Object.assign(this.hello, readHello(reply));
        return;
      } catch (e) {
        if (!isMissingHello(e)) throw e;
      }
    }

    // RESP2. `AUTH` on its own, in the two-argument form when there is an ACL
    // username and the one-argument form otherwise — the two-argument form is
    // itself a Redis 6 feature, so a server without `HELLO` will not take it.
    if (options.password !== undefined) {
      await this.#exchange(
        options.username === undefined || options.username === "default"
          ? ["AUTH", options.password]
          : ["AUTH", options.username, options.password],
      );
    }
    Object.assign(this.hello, { proto: 2 });
  }

  // -- the wire -------------------------------------------------------------

  /** Latches the first transport failure and tears the connection down. */
  #die(cause: unknown): DbError {
    if (this.#fatal === null) {
      const detail = cause instanceof Error ? cause.message : String(cause);
      this.#fatal = new DbError(`the connection to the server was lost: ${detail}`, {
        code: DbErrorCode.ConnectionLost,
        cause,
      });
      // Once a reply has been half-read, nothing later on this socket can be
      // trusted to start on a boundary. Every subsequent caller gets this same
      // error rather than a different symptom of the one dead connection.
      void this.#teardown();
    }
    return this.#fatal;
  }

  /**
   * One command out, one reply in, with the connection held for both.
   *
   * Not locked — `#call` is. This is the inner half, so the handshake can use
   * it before anything else could be contending.
   */
  async #exchange(args: readonly CommandArg[]): Promise<Reply> {
    if (this.#fatal !== null) throw this.#fatal;
    const writer = this.#writer;
    const replies = this.#replies;
    if (writer === null || replies === null) {
      throw new DbError("the connection is closed", { code: DbErrorCode.Closed });
    }
    // Encoded **before** the try. An argument this cannot serialize is the
    // caller's mistake and nothing has been written yet, so it must not reach
    // `#die` — latching a fatal error there would destroy a perfectly healthy
    // connection over a typo, and every later caller would inherit it.
    let bytes: Uint8Array;
    try {
      bytes = encodeCommand(args);
    } catch (e) {
      throw new DbError(e instanceof Error ? e.message : String(e), {
        code: DbErrorCode.Unsupported,
        cause: e,
      });
    }

    let reply: Reply;
    try {
      await writer.write(bytes);
      reply = await replies.next();
    } catch (e) {
      // A protocol error is as fatal as a dropped socket: both mean the reader
      // no longer knows where it is in the stream.
      throw this.#die(e);
    }
    if (reply.kind === "error") throw serverError(reply.value.prefix, reply.value.message);
    return reply;
  }

  /** Takes the connection for one exchange, and gives it back. */
  async #call(args: readonly CommandArg[]): Promise<Reply> {
    if (this.#fatal !== null) throw this.#fatal;
    let release: () => void = () => {};
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    const previous = this.#lock;
    this.#lock = held;
    // A failed exchange must not poison the ones behind it.
    await previous.catch(() => {});
    try {
      const reply = await this.#exchange(args);
      this.#observe(args);
      return reply;
    } finally {
      release();
    }
  }

  /**
   * Notes the commands that change what this connection *is*.
   *
   * Only after a successful reply: a `SELECT` the server refused did not move
   * anything, and recording it would condemn a perfectly good connection.
   */
  #observe(args: readonly CommandArg[]): void {
    const name = commandName(args);
    if (name === "SELECT") {
      const index = Number(args[1]);
      if (Number.isFinite(index)) this.#db = index;
    } else if (name === "MULTI") {
      this.#inMulti = true;
    } else if (name === "EXEC" || name === "DISCARD" || name === "RESET") {
      this.#inMulti = false;
    }
  }

  /**
   * Runs one command and returns the reply as ordinary JavaScript.
   *
   * The driver's own entry point, under `runtime:db`'s surface rather than
   * inside it — the client API in this package is built on this, and so is
   * anything a caller wants to send that has no helper.
   */
  async command(args: readonly CommandArg[], options: { signal?: AbortSignal } = {}): Promise<unknown> {
    this._open();
    const checked = guard(args);
    return this._withSignal(options.signal, async () => toValue(await this.#call(checked), this.#decode));
  }

  // -- the runtime:db surface ----------------------------------------------

  /**
   * The command an AST describes.
   *
   * `queryAst(["GET", key])` is the whole command. Positional parameters, if
   * any, are **appended** — `queryAst(["SET"])` with `["k", "v"]` is
   * `SET k v` — which is what makes `executeMany` mean something here: one
   * command and many argument sets, the same shape it has on a SQL backend.
   */
  #commandOf(q: NormalizedQuery): CommandArg[] {
    if (q.named.length > 0) {
      throw new DbError(
        "Redis commands take positional arguments only; pass an array rather than an object",
        { code: DbErrorCode.Unsupported },
      );
    }
    const ast = q.ast;
    if (!Array.isArray(ast) || ast.length === 0) {
      throw new DbError(
        "a redis query is a non-empty command array: queryAst([\"GET\", key])",
        { code: DbErrorCode.QueryForm },
      );
    }
    return guard([...ast, ...q.positional] as CommandArg[]);
  }

  protected async _query(q: NormalizedQuery): Promise<Rows> {
    const reply = await this.#call(this.#commandOf(q));
    const { columns, rows: total } = shapeOf(reply);
    const shape = defineRowShape(columns);
    const decode = this.#decode;
    let at = 0;
    return new Rows(
      {
        // Always. A RESP reply is complete once it has been read — there is no
        // cursor to leave open and nothing to close — so the connection is free
        // the moment this returns, which is exactly what a pool wants to know.
        exhausted: true,
        async next(maxBytes: number) {
          if (at >= total) return { bytes: EMPTY, rows: 0, done: true };
          const batch = writeRows(reply, at, total, maxBytes || BATCH_BYTES, decode);
          at += batch.rows;
          return batch;
        },
        async close(): Promise<void> {},
      },
      shape,
    );
  }

  protected async _execute(q: NormalizedQuery): Promise<ExecuteResult> {
    return asExecuteResult(await this.#call(this.#commandOf(q)));
  }

  // `_executeMany` is deliberately **not** overridden. Redis batches by
  // pipelining — many commands written before any reply is read — which is a
  // second exchange shape this connection does not have, so the base class's
  // loop is what runs: one round trip per set, correct and no faster. It is the
  // one place this driver leaves measurable time on the table, and it is named
  // in the README rather than left to be discovered.
  //
  // Note that the batch is **not atomic** here, where it is on a SQL backend:
  // `supports.transactions` is false, so `executeMany` does not wrap it in one.

  protected async _close(): Promise<void> {
    if (this.#fatal === null && this.#writer !== null) {
      try {
        // A polite goodbye. Redis closes on `QUIT` without waiting to be asked
        // twice, and it only makes sense on a connection that is still there.
        await this.#writer.write(encodeCommand(["QUIT"]));
      } catch {
        /* the peer may already be gone; the teardown below is what matters */
      }
    }
    await this.#teardown();
  }

  async #teardown(): Promise<void> {
    const [socket, replies, writer] = [this.#socket, this.#replies, this.#writer];
    this.#socket = null;
    this.#replies = null;
    this.#writer = null;
    try {
      writer?.releaseLock();
      await replies?.cancel();
      await socket?.close();
    } catch {
      /* closing twice is not an error */
    }
  }
}

const EMPTY = new Uint8Array(0);

/** The command's name, upper-cased, for the few decisions that need it. */
function commandName(args: readonly CommandArg[]): string {
  const first = args[0];
  return typeof first === "string" ? first.toUpperCase() : "";
}

/**
 * The commands that would change what arrives on the socket.
 *
 * Entering subscriber mode makes the server push messages that no caller is
 * waiting for, and `MONITOR` makes it push a copy of everything. This reader
 * expects one reply per command; either would leave it reading the wrong bytes
 * for the rest of the connection's life — a desynchronization that shows up
 * later, somewhere else, as a nonsense value.
 *
 * Refused by name, pointing at the feature that will support them, rather than
 * accepted into a state this release cannot represent.
 */
const MODE_CHANGING = new Set([
  "SUBSCRIBE",
  "UNSUBSCRIBE",
  "PSUBSCRIBE",
  "PUNSUBSCRIBE",
  "SSUBSCRIBE",
  "SUNSUBSCRIBE",
  "MONITOR",
]);

function guard(args: readonly CommandArg[]): CommandArg[] {
  if (args.length === 0) {
    throw new DbError("a redis command needs at least a name", { code: DbErrorCode.QueryForm });
  }
  const name = commandName(args);
  if (MODE_CHANGING.has(name)) {
    throw new DbError(
      `${name} puts the connection into a mode where the server pushes messages, which this release does not support — pub/sub is a separate feature, and running it here would desynchronize the reply stream`,
      { code: DbErrorCode.Unsupported },
    );
  }
  return args as CommandArg[];
}

/**
 * A reply, as `execute()` reports it.
 *
 * `changes` is the integer when Redis answered with one, and that integer is
 * **command-specific**: `DEL` returns keys removed, `SADD` members added,
 * `INCR` the new value. There is no generic count in Redis to report instead,
 * so this reports what was said rather than inventing a number — and the
 * command whose integer is not a count of changes is one to run through the
 * client API, where it is named for what it returns.
 *
 * `lastInsertRowid` is always `null`. Redis has no equivalent, the same way
 * PostgreSQL has none.
 */
function asExecuteResult(reply: Reply): ExecuteResult {
  switch (reply.kind) {
    case "integer": {
      const value = reply.value;
      // Past 2^53 it is not a count of anything; clamping is more honest than
      // a rounded double presented as exact.
      return {
        changes: value > BigInt(Number.MAX_SAFE_INTEGER) ? Number.MAX_SAFE_INTEGER : Number(value),
        lastInsertRowid: null,
      };
    }
    case "boolean":
      return { changes: reply.value ? 1 : 0, lastInsertRowid: null };
    case "null":
      // `SET … NX` that did not apply, and every other "nothing happened".
      return { changes: 0, lastInsertRowid: null };
    case "array":
    case "set":
    case "push":
      return { changes: reply.value.length, lastInsertRowid: null };
    case "map":
      return { changes: reply.value.length, lastInsertRowid: null };
    default:
      // `+OK`, a bulk string, a double: the command applied, once.
      return { changes: 1, lastInsertRowid: null };
  }
}

/** A `HELLO` reply, which is a map in RESP3 and a flat array in RESP2. */
function readHello(reply: Reply): Partial<ServerHello> {
  const value = toValue(reply, { binary: false });
  if (value !== null && typeof value === "object" && !Array.isArray(value)) {
    return value as Partial<ServerHello>;
  }
  if (Array.isArray(value)) {
    const out: Record<string, unknown> = {};
    for (let i = 0; i + 1 < value.length; i += 2) out[String(value[i])] = value[i + 1];
    return out as Partial<ServerHello>;
  }
  return {};
}

/**
 * Whether a failed `HELLO` means "this server has no RESP3" rather than
 * "authentication failed".
 *
 * Getting this wrong in either direction is bad: treating a wrong password as a
 * downgrade would connect an unauthenticated client and fail later somewhere
 * confusing, and treating an old server as an auth failure would make Redis 5
 * unreachable.
 */
function isMissingHello(e: unknown): boolean {
  if (!(e instanceof DbError)) return false;
  if (e.backendCode === "NOPROTO") return true;
  return e.backendCode === "ERR" && /unknown command/i.test(e.message);
}

function serverError(prefix: string, message: string): DbError {
  const text = prefix === "MOVED" || prefix === "ASK" ? redirectMessage(prefix, message) : message;
  return new DbError(text, {
    code: portableCode(prefix, message),
    backendCode: prefix,
  });
}
