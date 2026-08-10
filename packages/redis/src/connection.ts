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
  type DbInput,
  type ExecuteResult,
  type NormalizedQuery,
} from "runtime:db";

import { RespReader, encodeCommand, type CommandArg, type Reply } from "./protocol/resp.js";
import { blocksForever, foreverMessage } from "./protocol/blocking.js";
import { parseRedirect, portableCode, redirectMessage, type Redirect } from "./protocol/errors.js";
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
   * This connection exists to block, so `BLPOP key 0` and its relatives are
   * allowed on it. Default `false`.
   *
   * A blocking command holds the connection for as long as it blocks, and an
   * unbounded one therefore never gives it back — which is refused by default
   * because on a shared or pooled connection it is a hang nobody chose. Setting
   * this is the caller saying that tying *this* connection up is the point,
   * which is exactly how a queue worker is deployed.
   *
   * `createPool` strips it: a pool's premise is that its connections come back.
   */
  blocking?: boolean;
  /**
   * Reopen the connection after a transport failure. Default **off**.
   *
   * `true` takes the defaults; an object tunes them. Off by default because
   * turning it on changes what a thrown error means — with it, a failure that
   * reached your code is one the driver already gave up on — and because a
   * `Pool` does not need it: a pool replaces a dead connection with a new one,
   * which is reconnection with none of the state questions.
   *
   * What is restored is what is safe to restore: the handshake, the selected
   * database, the client name, and any subscriptions. What is **not**:
   *
   * - **The command that failed.** It was written, and whether the server ran
   *   it before the socket died is not knowable — replaying `INCR` would
   *   double-count. Its caller gets the error.
   * - **`WATCH`.** The server forgot it, so the optimistic lock it stood for is
   *   void. The next `EXEC` fails with `ERR_DB_SERIALIZATION_FAILURE` rather
   *   than succeeding on a guarantee nobody is making any more.
   * - **An open `MULTI`.** Its queued commands are gone with the connection.
   * - **Messages published while the connection was down.** Pub/sub has no
   *   queue and no delivery guarantee; a gap loses what was sent in it.
   */
  reconnect?: boolean | ReconnectOptions;
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

/** How hard to try, and how long to wait between attempts. */
export interface ReconnectOptions {
  /** How many times to try before giving up. Default 10; `0` means forever. */
  attempts?: number;
  /** The first wait, in ms. Doubles each attempt. Default 100. */
  delay?: number;
  /** The longest wait, in ms. Default 5000. */
  maxDelay?: number;
}

/** A published message's payload — bytes in `binary` mode, text otherwise. */
export type RedisPayload = string | Uint8Array;

/** Where a message came from. `pattern` only for a `psubscribe` delivery. */
export interface MessageContext {
  /** The channel the message was published to — always the concrete one. */
  readonly channel: string;
  /** The pattern that matched, for a `psubscribe` handler. */
  readonly pattern?: string;
}

/**
 * A subscription handler.
 *
 * It is called synchronously from the read loop, so it should not block: a slow
 * handler delays every later message on the connection. Hand the work to a
 * queue if it is not quick.
 */
export type MessageHandler = (payload: RedisPayload, context: MessageContext) => void;

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
  /** Whether this connection opted into blocking indefinitely. */
  #blocking = false;
  /** How this connection was opened, so it can be opened the same way again. */
  #target: RedisOptions = {};
  #reconnect: Required<ReconnectOptions> | null = null;
  /** The reconnection in progress, so concurrent callers share one attempt. */
  #reopening: Promise<void> | null = null;
  /**
   * The teardown a failure started, which reopening has to wait for.
   *
   * `#die` cannot await it — it is called from the middle of a failing
   * exchange — so it starts it and leaves the handle here. Dialing before that
   * finishes is a real race and not a theoretical one: the host recycles socket
   * ids, so the close of the old socket can land on the *new* one and kill the
   * connection that just replaced it.
   */
  #tearingDown: Promise<void> | null = null;
  /**
   * Set when a reconnection happened while `WATCH`es were outstanding.
   *
   * The server forgot them, so the optimistic lock they stood for is void. An
   * `EXEC` that went ahead would report success for a check nobody made.
   */
  #watchLost = false;
  /** Whether any `WATCH` is outstanding on this connection. */
  #watching = false;

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

  // -- subscriber state -----------------------------------------------------

  /** The read loop, once this connection has been given over to subscribing. */
  #pump: Promise<void> | null = null;
  /** Subscribe/unsubscribe confirmations still owed, oldest first. */
  #confirmations: { resolve: () => void; reject: (e: unknown) => void }[] = [];
  #channels = new Map<string, Set<MessageHandler>>();
  #patterns = new Map<string, Set<MessageHandler>>();
  #shards = new Map<string, Set<MessageHandler>>();
  /** Set while `_close` is tearing down, so the pump's failure is not news. */
  #closing = false;

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
    this.#blocking = options.blocking === true;
    this.#target = options;
    this.#reconnect = normalizeReconnect(options.reconnect);
    await this.#dial(options);
  }

  /** Opens the socket and does everything that makes it a usable connection. */
  async #dial(options: RedisOptions): Promise<void> {
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

  // -- reconnection ---------------------------------------------------------

  /** Whether this connection will try to reopen itself after a failure. */
  get reconnects(): boolean {
    return this.#reconnect !== null;
  }

  /**
   * Makes sure there is a live connection, reopening if one was lost.
   *
   * Called at the head of every exchange rather than eagerly from the failure,
   * so an idle connection nobody is using does not spend the process's life
   * reconnecting to a server that is down. A subscriber is the exception and
   * reconnects from its read loop, because nobody is going to call it.
   */
  async #ensureOpen(): Promise<void> {
    if (this.#fatal === null) return;
    if (this.#reconnect === null || this.#closing) throw this.#fatal;
    // One attempt shared by every waiting caller: a burst of commands arriving
    // after a server restart should reopen the connection once, not once each.
    this.#reopening ??= this.#reopen().finally(() => {
      this.#reopening = null;
    });
    await this.#reopening;
  }

  async #reopen(): Promise<void> {
    const plan = this.#reconnect;
    if (plan === null) throw this.#fatal ?? new DbError("no connection", { code: DbErrorCode.Closed });
    const previous = this.#fatal;
    let wait = plan.delay;

    for (let attempt = 1; plan.attempts === 0 || attempt <= plan.attempts; attempt++) {
      await new Promise((resolve) => setTimeout(resolve, wait));
      wait = Math.min(wait * 2, plan.maxDelay);
      if (this.#closing) throw previous ?? new DbError("closed", { code: DbErrorCode.Closed });
      // The failure's own teardown first, then anything left of ours.
      await this.#tearingDown?.catch(() => {});
      this.#tearingDown = null;
      await this.#teardown();
      try {
        this.#fatal = null;
        await this.#dial(this.#target);
      } catch {
        // Left latched for the next attempt; the loop is what decides to stop.
        this.#fatal = previous;
        continue;
      }
      // A WATCH the old connection held is gone with it, and the server has no
      // memory of it. Recorded rather than silently forgotten, because an EXEC
      // that proceeded would be reporting a guarantee nobody is making.
      if (this.#watching) {
        this.#watchLost = true;
        this.#watching = false;
      }
      // An open MULTI went with the connection too.
      this.#inMulti = false;
      await this.#resubscribe();
      return;
    }
    this.#fatal = previous;
    throw previous ?? new DbError("the connection could not be reopened", {
      code: DbErrorCode.ConnectionLost,
    });
  }

  /**
   * Puts the subscriptions back after a reconnect.
   *
   * Safe to replay in a way an ordinary command is not: subscribing twice is
   * subscribing once. What cannot be recovered is anything published while the
   * connection was down — pub/sub has no queue, so a gap loses what was sent in
   * it, and a caller that cannot tolerate that wants a stream rather than a
   * channel.
   */
  async #resubscribe(): Promise<void> {
    const groups: [string, Map<string, Set<MessageHandler>>][] = [
      ["SUBSCRIBE", this.#channels],
      ["PSUBSCRIBE", this.#patterns],
      ["SSUBSCRIBE", this.#shards],
    ];
    const wanted = groups.filter(([, registry]) => registry.size > 0);
    if (wanted.length === 0) return;
    // The pump is gone with the old socket; a fresh one reads the new
    // confirmations and everything after them.
    this.#pump = null;
    this.#confirmations.length = 0;
    this.#startPump();
    for (const [command, registry] of wanted) {
      const names = [...registry.keys()];
      await this.#confirmed(names.length, [command, ...names]);
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

    try {
      await writer.write(bytes);
    } catch (e) {
      // Nothing reached the server, so this one is safe to run again on a
      // reopened connection.
      const failure = this.#die(e);
      UNSENT.add(failure);
      throw failure;
    }

    let reply: Reply;
    try {
      reply = await replies.next();
    } catch (e) {
      // A protocol error is as fatal as a dropped socket: both mean the reader
      // no longer knows where it is in the stream. And this one *was* sent.
      throw this.#die(e);
    }
    if (reply.kind === "error") throw serverError(reply.value.prefix, reply.value.message);
    return reply;
  }

  /**
   * Writes many commands, then reads their replies in order.
   *
   * The pipelining primitive: N commands cost one round trip instead of N,
   * because the replies are already on their way back while the later commands
   * are still being written. It holds the connection for the whole batch, which
   * is what makes matching reply *i* to command *i* safe — the lock is the only
   * thing guaranteeing nobody else's reply is in between.
   *
   * Error replies are **returned, not thrown**. In a pipeline each command
   * stands alone, so one failing says nothing about the others, and the caller
   * is the only layer that knows what to do about that.
   */
  async #pipeline(
    commands: readonly (readonly CommandArg[])[],
    { retry = true }: { retry?: boolean } = {},
  ): Promise<Reply[]> {
    await this.#ensureOpen();
    if (this.#fatal !== null) throw this.#fatal;
    if (this.#pump !== null) {
      throw new DbError(
        "this connection is subscribed and runs no commands — use another connection, or a pool",
        { code: DbErrorCode.ConnectionBusy },
      );
    }
    // Encoded before the lock is taken: an argument that cannot be serialized is
    // the caller's mistake, and nothing should be written or held for it.
    const payloads = commands.map((args) => {
      try {
        return encodeCommand(args);
      } catch (e) {
        throw new DbError(e instanceof Error ? e.message : String(e), {
          code: DbErrorCode.Unsupported,
          cause: e,
        });
      }
    });

    let release: () => void = () => {};
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    const previous = this.#lock;
    this.#lock = held;
    await previous.catch(() => {});

    try {
      // Re-read on every attempt: a retry runs after a reconnect, and the
      // writer and reader it should use are the new socket's.
      const out = await this.#retrying(async () => {
        const writer = this.#writer;
        const replies = this.#replies;
        if (writer === null || replies === null) {
          throw new DbError("the connection is closed", { code: DbErrorCode.Closed });
        }
        try {
          // One write for the whole batch. Writing them separately would be
          // correct and would give up most of what pipelining buys.
          await writer.write(concat(payloads));
        } catch (e) {
          const failure = this.#die(e);
          UNSENT.add(failure);
          throw failure;
        }
        const replied: Reply[] = [];
        try {
          for (let i = 0; i < commands.length; i++) replied.push(await replies.next());
        } catch (e) {
          // A batch that failed part-way has left replies on the wire that
          // nobody will read, so the stream can no longer be trusted to start
          // on a boundary. That is fatal, exactly as a dropped socket is.
          throw this.#die(e);
        }
        return replied;
      }, retry);
      for (const args of commands) this.#observe(args);
      return out;
    } finally {
      release();
    }
  }

  /**
   * Runs `commands` inside `MULTI`/`EXEC`.
   *
   * Sent as one batch — `MULTI`, every command, `EXEC` — so the whole
   * transaction is one round trip rather than one per queued command.
   *
   * Answers `null` when `EXEC` was aborted because a `WATCH`ed key changed,
   * which is the optimistic-locking outcome and not an error. Throws when the
   * transaction was **discarded** by the server: a command rejected at queue
   * time (a bad argument count, an unknown command) makes `EXEC` fail with
   * `EXECABORT` and nothing runs at all — the one case Redis does undo the lot.
   *
   * Otherwise the array is one entry per command, and an entry may itself be a
   * `DbError`: a command that fails at *execution* time does not roll back the
   * ones beside it, and reporting that as a thrown error would discard the
   * results of the commands that did apply.
   */
  async execTransaction(commands: readonly (readonly CommandArg[])[]): Promise<unknown[] | null> {
    this._open();
    if (commands.length === 0) return [];
    if (this.#watchLost) {
      // Reported once. The caller now knows to redo the whole read-and-compare
      // cycle, and leaving the flags set would refuse their retry as well.
      this.#watchLost = false;
      this.#watching = false;
      throw new DbError(
        "this connection was reopened while a WATCH was outstanding, so the server no longer holds it — the transaction was not sent. Read the watched keys again and retry.",
        { code: DbErrorCode.SerializationFailure },
      );
    }
    const checked = commands.map((args) => guard(args, this.#blocking));
    // A `WATCH` lives on the server, on *this* connection. If sending fails and
    // the connection is reopened, the watch is gone — so this must not take the
    // ordinary unsent-retry, which would re-send the transaction onto a
    // connection holding no watch and report success for a check nobody made.
    const watching = this.#watching;
    let replies: Reply[];
    try {
      replies = await this.#pipeline([["MULTI"], ...checked, ["EXEC"]], { retry: !watching });
    } catch (e) {
      if (!watching) throw e;
      this.#watchLost = false;
      this.#watching = false;
      throw new DbError(
        "the connection was lost while a WATCH was outstanding, so the transaction was not applied and the watch is void — read the watched keys again and retry",
        { code: DbErrorCode.SerializationFailure, cause: e },
      );
    }
    const exec = replies[replies.length - 1]!;

    if (exec.kind === "error") {
      // EXECABORT, almost always — one of the commands was refused as it was
      // queued, so the server threw the whole transaction away.
      const queueError = replies.slice(1, -1).find((r) => r.kind === "error");
      const detail =
        queueError !== undefined && queueError.kind === "error"
          ? ` — ${queueError.value.message}`
          : "";
      throw serverError(exec.value.prefix, `${exec.value.message}${detail}`);
    }
    // `EXEC` answers a null reply when a WATCHed key was touched by someone
    // else. Nothing ran, and the caller is meant to read again and retry.
    if (exec.kind === "null") return null;
    if (exec.kind !== "array" && exec.kind !== "push") {
      throw new DbError(`EXEC answered a ${exec.kind}, which is not a result list`, {
        code: DbErrorCode.Backend,
      });
    }
    return exec.value.map((reply) =>
      reply.kind === "error"
        ? serverError(reply.value.prefix, reply.value.message)
        : toValue(reply, this.#decode),
    );
  }

  /**
   * Runs many commands as a pipeline, answering one result per command.
   *
   * No atomicity and none implied: another client's commands may land among
   * these, and one failing does not stop the rest. What it buys is the round
   * trips — N commands cost one instead of N.
   *
   * A failed command's result is a `DbError` **in place** rather than a throw,
   * for the same reason a transaction's is: the others ran, and throwing would
   * discard their results.
   */
  async execPipeline(commands: readonly (readonly CommandArg[])[]): Promise<unknown[]> {
    this._open();
    if (commands.length === 0) return [];
    if (this.#watchLost) {
      // Reported once. The caller now knows to redo the whole read-and-compare
      // cycle, and leaving the flags set would refuse their retry as well.
      this.#watchLost = false;
      this.#watching = false;
      throw new DbError(
        "this connection was reopened while a WATCH was outstanding, so the server no longer holds it — the transaction was not sent. Read the watched keys again and retry.",
        { code: DbErrorCode.SerializationFailure },
      );
    }
    const checked = commands.map((args) => guard(args, this.#blocking));
    const replies = await this.#pipeline(checked);
    return replies.map((reply) =>
      reply.kind === "error"
        ? serverError(reply.value.prefix, reply.value.message)
        : toValue(reply, this.#decode),
    );
  }

  /** Takes the connection for one exchange, and gives it back. */
  async #call(args: readonly CommandArg[]): Promise<Reply> {
    await this.#ensureOpen();
    if (this.#fatal !== null) throw this.#fatal;
    if (this.#pump !== null) {
      // The read loop owns the reader, so there is nobody to hand a reply to.
      // Over RESP2 the server would refuse the command anyway — a subscribed
      // connection accepts only the subscribe family — so this refuses for the
      // protocol's reason as much as for the implementation's.
      throw new DbError(
        "this connection is subscribed and runs no commands — use another connection, or a pool",
        { code: DbErrorCode.ConnectionBusy },
      );
    }
    let release: () => void = () => {};
    const held = new Promise<void>((resolve) => {
      release = resolve;
    });
    const previous = this.#lock;
    this.#lock = held;
    // A failed exchange must not poison the ones behind it.
    await previous.catch(() => {});
    try {
      const reply = await this.#retrying(() => this.#exchange(args));
      this.#observe(args);
      return reply;
    } finally {
      release();
    }
  }

  /**
   * Runs `work`, and runs it once more if the connection had gone away before
   * it sent anything.
   *
   * The retry is bounded to one and to the unsent case. Without it a server
   * restart costs every live connection one spurious error, because nothing
   * notices a socket has closed until something tries to use it — and the
   * command that discovers it did not deserve to be the one that fails.
   */
  async #retrying<T>(work: () => Promise<T>, retry = true): Promise<T> {
    try {
      return await work();
    } catch (e) {
      if (
        !retry ||
        this.#reconnect === null ||
        this.#closing ||
        typeof e !== "object" ||
        e === null ||
        !UNSENT.has(e)
      ) {
        throw e;
      }
      await this.#ensureOpen();
      return work();
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
      this.#watching = false;
    } else if (name === "WATCH") {
      this.#watching = true;
    } else if (name === "UNWATCH") {
      this.#watching = false;
      // The caller has said they no longer depend on the lost watch, so the
      // connection stops answering for it.
      this.#watchLost = false;
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
    const checked = guard(args, this.#blocking);
    return this._withSignal(options.signal, async () => toValue(await this.#call(checked), this.#decode));
  }

  // -- pub/sub --------------------------------------------------------------

  /**
   * Called for every message on a channel this connection is subscribed to,
   * after any handler registered for that channel specifically.
   *
   * The catch-all, for a caller that would rather switch on the channel itself
   * than register per channel.
   */
  onMessage: MessageHandler | undefined;

  /** Called when the read loop fails, since nobody is awaiting it. */
  onSubscribeError: ((error: unknown) => void) | undefined;

  /** Whether this connection may block indefinitely — `{ blocking: true }`. */
  get blocking(): boolean {
    return this.#blocking;
  }

  /** Whether this connection has been given over to subscribing. */
  get subscribed(): boolean {
    return this.#pump !== null;
  }

  /** The channels, patterns and shard channels currently subscribed. */
  get channels(): string[] {
    return [...this.#channels.keys()];
  }

  get patterns(): string[] {
    return [...this.#patterns.keys()];
  }

  get shardChannels(): string[] {
    return [...this.#shards.keys()];
  }

  /**
   * Subscribes to one or more channels.
   *
   * **The first subscribe gives this connection over to a read loop**, and from
   * then on it runs no ordinary commands: `query`, `execute` and `command`
   * refuse with `ERR_DB_CONNECTION_BUSY`. That is not a simplification — over
   * RESP2 it is the protocol's own rule, since a subscribed connection accepts
   * nothing but the subscribe family — and it is how you would deploy it
   * anyway: a connection that must notice a message promptly should not be
   * waiting behind someone's report query.
   *
   * The loop owns *reading*. A `SUBSCRIBE` only needs *writing*, and TCP is
   * full duplex, so the command goes out underneath the loop and the loop
   * resolves it when its confirmation comes back — which is why this can await
   * confirmation rather than hope, and why a subscribe that the server refuses
   * fails here instead of silently never firing.
   *
   * A connection given over to subscribing **stays** a subscriber. Unsubscribing
   * from everything stops the messages, not the mode; open another connection
   * for ordinary work, which is what a subscriber wants anyway.
   */
  async subscribe(channels: string | readonly string[], handler?: MessageHandler): Promise<void> {
    await this.#subscribeTo("SUBSCRIBE", this.#channels, channels, handler);
  }

  /** Subscribes by glob pattern — `news.*`. Messages carry the pattern too. */
  async psubscribe(patterns: string | readonly string[], handler?: MessageHandler): Promise<void> {
    await this.#subscribeTo("PSUBSCRIBE", this.#patterns, patterns, handler);
  }

  /** Subscribes to a sharded channel (Redis 7+), which a cluster does not broadcast. */
  async ssubscribe(channels: string | readonly string[], handler?: MessageHandler): Promise<void> {
    await this.#subscribeTo("SSUBSCRIBE", this.#shards, channels, handler);
  }

  /** Unsubscribes. With no argument, from every channel. */
  async unsubscribe(channels?: string | readonly string[]): Promise<void> {
    await this.#unsubscribeFrom("UNSUBSCRIBE", this.#channels, channels);
  }

  async punsubscribe(patterns?: string | readonly string[]): Promise<void> {
    await this.#unsubscribeFrom("PUNSUBSCRIBE", this.#patterns, patterns);
  }

  async sunsubscribe(channels?: string | readonly string[]): Promise<void> {
    await this.#unsubscribeFrom("SUNSUBSCRIBE", this.#shards, channels);
  }

  async #subscribeTo(
    command: string,
    registry: Map<string, Set<MessageHandler>>,
    names: string | readonly string[],
    handler?: MessageHandler,
  ): Promise<void> {
    this._open();
    const list = typeof names === "string" ? [names] : [...names];
    if (list.length === 0) return;
    this.#startPump();
    // Registered before the confirmation rather than after: the server may
    // deliver a message the instant it accepts the subscription, and a handler
    // added afterwards would miss it.
    for (const name of list) {
      const handlers = registry.get(name) ?? new Set();
      if (handler !== undefined) handlers.add(handler);
      registry.set(name, handlers);
    }
    try {
      // One confirmation per name, which is what the server sends.
      await this.#confirmed(list.length, [command, ...list]);
    } catch (e) {
      for (const name of list) {
        const handlers = registry.get(name);
        if (handlers === undefined) continue;
        if (handler !== undefined) handlers.delete(handler);
        if (handlers.size === 0) registry.delete(name);
      }
      throw e;
    }
  }

  async #unsubscribeFrom(
    command: string,
    registry: Map<string, Set<MessageHandler>>,
    names?: string | readonly string[],
  ): Promise<void> {
    this._open();
    if (this.#pump === null) return;
    const list = names === undefined ? [...registry.keys()] : typeof names === "string" ? [names] : [...names];
    if (list.length === 0) return;
    await this.#confirmed(list.length, [command, ...list]);
    for (const name of list) registry.delete(name);
  }

  /** Writes a subscribe-family command and waits for `count` confirmations. */
  #confirmed(count: number, args: readonly CommandArg[]): Promise<void> {
    return new Promise<void>((resolve, reject) => {
      let left = count;
      const one = {
        resolve: () => {
          if (--left === 0) resolve();
        },
        reject,
      };
      for (let i = 0; i < count; i++) this.#confirmations.push(one);
      this.#write(args).catch(reject);
    });
  }

  /** Writes without reading — the pump owns the reader. */
  async #write(args: readonly CommandArg[]): Promise<void> {
    if (this.#fatal !== null) throw this.#fatal;
    const writer = this.#writer;
    if (writer === null) {
      throw new DbError("the connection is closed", { code: DbErrorCode.Closed });
    }
    try {
      await writer.write(encodeCommand(args));
    } catch (e) {
      throw this.#die(e);
    }
  }

  #startPump(): void {
    if (this.#pump !== null) return;
    this.#pump = (async () => {
      for (;;) {
        let reply: Reply;
        try {
          const replies = this.#replies;
          if (replies === null) return;
          reply = await replies.next();
        } catch (e) {
          if (this.#closing) return;
          const error = this.#die(e);
          // Nobody is awaiting this loop, so a failure has to be handed
          // somewhere or it would be an unhandled rejection reported against a
          // line that did not cause it.
          for (const pending of this.#confirmations.splice(0)) pending.reject(error);
          this.onSubscribeError?.(error);
          // A subscriber has to reconnect itself: nobody is going to call it,
          // so the lazy path every other command takes would never run.
          if (this.#reconnect !== null && !this.#closing) {
            this.#reopening ??= this.#reopen().finally(() => {
              this.#reopening = null;
            });
            this.#reopening.catch((failure) => this.onSubscribeError?.(failure));
          }
          return;
        }
        this.#deliver(reply);
      }
    })();
  }

  /** Routes one thing the pump read: a message, a confirmation, or an error. */
  #deliver(reply: Reply): void {
    if (reply.kind === "error") {
      const error = serverError(reply.value.prefix, reply.value.message);
      this.#confirmations.shift()?.reject(error);
      return;
    }
    // RESP3 sends these as push frames and RESP2 as ordinary arrays. Same
    // content either way, so the only difference is which type byte carried it.
    const items = reply.kind === "push" || reply.kind === "array" ? reply.value : null;
    if (items === null || items.length === 0) {
      this.#confirmations.shift()?.resolve();
      return;
    }
    const kind = String(toValue(items[0]!, { binary: false })).toLowerCase();
    switch (kind) {
      case "subscribe":
      case "psubscribe":
      case "ssubscribe":
      case "unsubscribe":
      case "punsubscribe":
      case "sunsubscribe":
        this.#confirmations.shift()?.resolve();
        return;
      case "message":
      case "smessage": {
        const channel = String(toValue(items[1]!, { binary: false }));
        const payload = toValue(items[2]!, this.#decode) as RedisPayload;
        this.#dispatch(kind === "message" ? this.#channels : this.#shards, channel, payload, {
          channel,
        });
        return;
      }
      case "pmessage": {
        const pattern = String(toValue(items[1]!, { binary: false }));
        const channel = String(toValue(items[2]!, { binary: false }));
        const payload = toValue(items[3]!, this.#decode) as RedisPayload;
        this.#dispatch(this.#patterns, pattern, payload, { channel, pattern });
        return;
      }
      default:
        // Anything else on a subscribed connection is a reply to something the
        // subscribe API sent — a `PING` in RESP2's subscriber mode answers with
        // an array, not `+PONG`.
        this.#confirmations.shift()?.resolve();
    }
  }

  #dispatch(
    registry: Map<string, Set<MessageHandler>>,
    key: string,
    payload: RedisPayload,
    context: MessageContext,
  ): void {
    for (const handler of registry.get(key) ?? []) this.#safely(handler, payload, context);
    if (this.onMessage !== undefined) this.#safely(this.onMessage, payload, context);
  }

  /**
   * A handler that throws must not take the read loop with it.
   *
   * The loop is the only thing reading this socket; letting one bad handler end
   * it would silently stop every other subscription on the connection.
   */
  #safely(handler: MessageHandler, payload: RedisPayload, context: MessageContext): void {
    try {
      handler(payload, context);
    } catch (e) {
      this.onSubscribeError?.(e);
    }
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
    return guard([...ast, ...q.positional] as CommandArg[], this.#blocking);
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

  /**
   * The batch path: one round trip for the whole set, by pipelining.
   *
   * The base class's default loops `_execute`, which is correct and pays a
   * round trip per set — and a round trip is the whole cost of a Redis command,
   * so that loop spends its time on the network rather than in Redis.
   *
   * Two differences from a SQL backend's batch, both following from Redis
   * rather than from a choice here. It is **not atomic**:
   * `supports.transactions` is false, so `executeMany` does not wrap it in a
   * transaction and a failure part-way leaves the earlier sets applied. And
   * every set is *attempted* — where the default loop stops at the first
   * failure, a pipeline has already sent them all — so a failure reports what
   * went wrong after the rest have run.
   */
  protected override async _executeMany(
    query: NormalizedQuery,
    sets: [DbInput[], [string, DbInput][]][],
  ): Promise<ExecuteResult> {
    if (query.named.length > 0 || sets.some(([, named]) => named.length > 0)) {
      throw new DbError(
        "Redis commands take positional arguments only; pass arrays rather than objects",
        { code: DbErrorCode.Unsupported },
      );
    }
    const ast = query.ast;
    if (!Array.isArray(ast) || ast.length === 0) {
      throw new DbError(
        'a redis query is a non-empty command array: queryAst(["SET"])',
        { code: DbErrorCode.QueryForm },
      );
    }
    const commands = sets.map(([positional]) =>
      guard([...ast, ...positional] as CommandArg[], this.#blocking),
    );
    const replies = await this.#pipeline(commands);

    let changes = 0;
    let failure: DbError | null = null;
    for (const reply of replies) {
      if (reply.kind === "error") {
        // The first failure is the one reported; the rest ran regardless, and
        // their results are counted so `changes` says what actually happened.
        failure ??= serverError(reply.value.prefix, reply.value.message);
        continue;
      }
      changes += asExecuteResult(reply).changes;
    }
    if (failure !== null) throw failure;
    return { changes, lastInsertRowid: null };
  }

  protected async _close(): Promise<void> {
    // The pump is parked on a read that tearing down is about to break. Told
    // first, so it treats that as the close it is rather than as a lost
    // connection worth reporting to `onSubscribeError`.
    this.#closing = true;
    for (const pending of this.#confirmations.splice(0)) {
      pending.reject(new DbError("the connection is closed", { code: DbErrorCode.Closed }));
    }
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

/**
 * Errors raised **before** anything reached the server.
 *
 * The distinction reconnection turns on. A command whose write failed was never
 * sent, so running it again cannot repeat it — and the host invalidates a socket
 * handle when the peer goes away, so a write to a connection the server closed
 * fails rather than succeeding into nothing. That covers the ordinary case: a
 * server restart, a `CLIENT KILL`, an idle timeout at the far end.
 *
 * A command whose *reply* never arrived is the opposite: it was written, and
 * whether the server ran it first is not knowable. Retrying `INCR` there would
 * double-count, so that one fails and its caller decides.
 *
 * A `WeakSet` rather than a property on the error, so the distinction stays an
 * implementation detail instead of becoming API nobody meant to promise.
 */
const UNSENT = new WeakSet<object>();

/** The reconnection plan, or `null` for a connection that stays dead. */
function normalizeReconnect(
  option: boolean | ReconnectOptions | undefined,
): Required<ReconnectOptions> | null {
  if (option === undefined || option === false) return null;
  const given = option === true ? {} : option;
  return {
    attempts: given.attempts ?? 10,
    delay: Math.max(1, given.delay ?? 100),
    maxDelay: Math.max(1, given.maxDelay ?? 5000),
  };
}

/** Joins encoded commands into the single write a pipeline sends. */
function concat(parts: readonly Uint8Array[]): Uint8Array {
  let size = 0;
  for (const part of parts) size += part.length;
  const out = new Uint8Array(size);
  let at = 0;
  for (const part of parts) {
    out.set(part, at);
    at += part.length;
  }
  return out;
}

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
]);

function guard(args: readonly CommandArg[], blocking: boolean): CommandArg[] {
  if (args.length === 0) {
    throw new DbError("a redis command needs at least a name", { code: DbErrorCode.QueryForm });
  }
  const name = commandName(args);
  if (MODE_CHANGING.has(name)) {
    throw new DbError(
      `send ${name} through the subscribe API — connection.${name.toLowerCase()}(channels, handler) — rather than as a raw command: the subscribe family is answered by a read loop that has to know about it, and a raw one would leave its confirmation and every later message unread`,
      { code: DbErrorCode.Unsupported },
    );
  }
  if (name === "MONITOR") {
    throw new DbError(
      "MONITOR turns the connection into a firehose of every command the server runs, which this reader — one reply per command — cannot represent. Use redis-cli for it.",
      { code: DbErrorCode.Unsupported },
    );
  }
  // A *bounded* blocking command is allowed: it holds the connection for its
  // timeout, which is a cost the caller chose. An unbounded one never gives it
  // back, which is not a cost anyone chose knowingly.
  const forever = blocksForever(args);
  if (forever !== null && !blocking) {
    throw new DbError(foreverMessage(forever), { code: DbErrorCode.Unsupported });
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
  const redirect = parseRedirect(prefix, message);
  // A redirect keeps the server's own words *and* gains the advice, because the
  // two audiences differ: `RedisCluster` reads `redirect` and follows it, while
  // a human who reached this on a single connection needs to be told that this
  // driver's cluster support is a different entry point rather than absent.
  const text = redirect === null ? message : redirectMessage(prefix, message);
  const error = new DbError(text, {
    code: portableCode(prefix, message),
    backendCode: prefix,
  });
  return redirect === null ? error : Object.assign(error, { redirect });
}

/** The redirect a `MOVED`/`ASK` failure carries, for whoever can follow it. */
export type { Redirect };
