/**
 * A PostgreSQL connection: the wire protocol, in JavaScript, over
 * `runtime:net`.
 *
 * There is no native code anywhere in this package. The transport is a socket
 * and a TLS upgrade; everything above it — framing, authentication, the
 * extended query protocol, row decoding — is here, which is the arrangement
 * `runtime:db` exists to make possible (DECISIONS D56).
 */
import { connect as netConnect } from "runtime:net";
import {
  BaseConnection,
  DbError,
  DbErrorCode,
  Dialect,
  Rows,
  asDbError,
  defineRowShape,
  decodeBatch,
} from "runtime:db";

import { Fields, FrameReader } from "./protocol/frame.js";

/** What `runtime:net`'s `connect()` hands back. */
type PgSocket = ReturnType<typeof netConnect>;
import * as msg from "./protocol/messages.js";
import { AUTH, B } from "./protocol/messages.js";
import { scram } from "./protocol/scram.js";
import { portableCode, type ServerMessage } from "./protocol/errors.js";
import { decoderFor, decoderForFormat, encodeParam, prefersBinary } from "./protocol/values.js";

/** A result set's shape, as `RowDescription` reports it. */
interface Columns {
  names: string[];
  oids: number[];
}

/** A statement prepared on the server, and what its rows will look like. */
interface Prepared {
  name: string;
  /** `null` when the statement returns no rows. */
  columns: Columns | null;
  /** Result format per column; empty means text throughout. */
  formats: number[];
}

/** Reads a `RowDescription` body into names and type OIDs. */
function readRowDescription(fields: Fields): Columns {
  const count = fields.i16();
  const names: string[] = [];
  const oids: number[] = [];
  for (let i = 0; i < count; i++) {
    names.push(fields.cstring());
    fields.i32(); // table oid
    fields.i16(); // column attribute number
    oids.push(fields.i32());
    fields.i16(); // type size
    fields.i32(); // type modifier
    fields.i16(); // format code
  }
  return { names, oids };
}

/** How much of a result set to gather before handing it to the caller. */
const BATCH_BYTES = 64 * 1024;

export const POSTGRES_DIALECT = new Dialect({
  name: "postgres",
  placeholder: (index) => `$${index}`,
  supports: {
    returning: true,
    savepoints: true,
    // The wire protocol binds by position only. `:name` never reaches the
    // server, so claiming it would mean rewriting SQL here — which means
    // parsing SQL here, which is the thing a driver should not do.
    namedParameters: false,
  },
});

export interface PgOptions {
  host?: string;
  port?: number;
  user?: string;
  password?: string;
  database?: string;
  applicationName?: string;
  /** `"prefer"` (default) asks for TLS; `"require"` insists; `"disable"` never asks. */
  sslmode?: "require" | "prefer" | "disable";
  /**
   * How long to wait for the connection **and its handshake**, in
   * milliseconds. Default 10 000; `0` waits forever.
   *
   * This is the bound that matters, because a server which completes the TCP
   * handshake and then says nothing is indistinguishable from a slow one, and
   * without a deadline the wait is unbounded. The URL spells it
   * `connect_timeout` in **seconds**, which is libpq's spelling and what every
   * connection string in the wild carries.
   */
  connectTimeout?: number;
  /**
   * `statement_timeout`, in milliseconds, applied to every statement on this
   * connection. Default unset (no limit).
   *
   * Sent as a startup parameter, so the **server** enforces it. A client-side
   * timer cannot: it would fire on a query the server is still working on, and
   * abandoning a connection mid-statement leaves it unusable. The server
   * cancels the statement and stays connected, which is the outcome worth
   * having.
   */
  statementTimeout?: number;
  /**
   * How many prepared statements to keep per connection. Default 100; `0`
   * disables caching.
   *
   * Without it every query re-parses its SQL, which for a statement run a
   * thousand times is a thousand parses of the same text. The bound matters as
   * much as the cache: each entry is a plan the server holds, so an application
   * generating unique SQL — a query builder inlining a different literal each
   * time — would otherwise accumulate them until the backend ran out of memory.
   */
  preparedStatementCacheSize?: number;
  /**
   * A certificate authority to trust in addition to the public roots, as PEM.
   *
   * The case this exists for is the ordinary one: an internal PostgreSQL
   * presenting a certificate from a private authority. Without it such a server
   * cannot be reached at all, because the public roots have never heard of it.
   * The URL spells it `sslrootcert`, matching libpq — though libpq takes a
   * *path* there and this takes the certificate itself, since reading a file
   * needs a capability a connection string should not silently exercise.
   */
  sslRootCert?: string | Uint8Array;
}

interface Batch {
  bytes: Uint8Array;
  rows: number;
  done: boolean;
}

export class PgConnection extends BaseConnection {
  #socket: PgSocket | null = null;
  #frames: FrameReader | null = null;
  #writer: WritableStreamDefaultWriter<Uint8Array> | null = null;
  /** Server parameters from the handshake (`server_version`, and so on). */
  readonly parameters: Record<string, string> = {};
  /** The last `ReadyForQuery` status: `I` idle, `T` in a transaction, `E` failed. */
  status = "I";

  /**
   * Whether this connection is still worth handing to anyone.
   *
   * False once a transport failure has been latched — a pool needs to ask
   * before offering one, because a connection can die while nobody is holding
   * it and the first anyone hears of that is otherwise the next caller's error.
   */
  get usable(): boolean {
    return this.#fatal === null && this.#socket !== null;
  }
  /**
   * One exchange at a time: a connection is a single conversation.
   *
   * The lock is held for the **whole** of an exchange, which for a query means
   * until its last row has been read — not until the first batch arrives.
   * Anything less lets a second query write its `Parse` onto the wire while the
   * first result is still coming back, and then two readers take turns on one
   * socket. That does not corrupt the stream so much as stop it: both sides
   * wait for the other's message and neither arrives.
   */
  #lock: Promise<unknown> = Promise.resolve();
  /**
   * Set while a result set is open and unread.
   *
   * A second exchange started here cannot simply queue. An in-flight `execute`
   * finishes on its own, so waiting for it takes as long as it takes; an open
   * result set finishes only when the **caller** drains it, and a caller who is
   * waiting on the queue never will. `for await (row of a) { await query(b) }`
   * is the shape, it is an ordinary thing to write, and queueing turns it from
   * a hang into a slower hang. So it is refused, by name, with the fix in the
   * message.
   */
  #streaming = false;

  constructor() {
    super({ dialect: POSTGRES_DIALECT, backend: "postgres" });
  }

  // -- lifecycle ------------------------------------------------------------

  async open(options: PgOptions): Promise<void> {
    const budget = options.connectTimeout ?? 10_000;
    if (budget <= 0) return this.#open(options);
    let timer: ReturnType<typeof setTimeout> | undefined;
    const expired = new Promise<never>((_, reject) => {
      timer = setTimeout(() => {
        reject(
          new DbError(
            `the connection to ${options.host ?? "localhost"}:${options.port ?? 5432} did not complete within ${budget}ms`,
            { code: DbErrorCode.Timeout },
          ),
        );
      }, budget);
    });
    try {
      await Promise.race([this.#open(options), expired]);
    } catch (e) {
      // The socket may be half-open behind a rejected race, and a descriptor
      // left dangling on a timeout is how a retry loop runs a process out of
      // them.
      await this._close().catch(() => {});
      throw e;
    } finally {
      clearTimeout(timer);
    }
  }

  /** Where and how to reach this server, kept so a cancel can reach it too. */
  #target: PgOptions = {};

  async #open(options: PgOptions): Promise<void> {
    this.#target = options;
    const dialled = await this.#dial(options);
    this.#socket = dialled.socket;
    this.#frames = dialled.frames ?? new FrameReader(dialled.socket.readable);
    this.#writer = dialled.socket.writable.getWriter();

    const params: Record<string, string> = {
      user: options.user ?? "postgres",
      database: options.database ?? options.user ?? "postgres",
      application_name: options.applicationName ?? "esrun",
      client_encoding: "UTF8",
    };
    if (options.preparedStatementCacheSize !== undefined) {
      this.#cacheLimit = Math.max(0, Math.trunc(options.preparedStatementCacheSize));
    }
    if (options.statementTimeout !== undefined && options.statementTimeout > 0) {
      // A GUC in the startup packet: in force from the first statement, with no
      // extra round trip and no window where it is not yet set.
      params["statement_timeout"] = String(Math.trunc(options.statementTimeout));
    }
    await this.#send(msg.startup(params));
    await this.#authenticate(options);
  }

  /**
   * Opens a socket to the server and negotiates TLS, speaking no protocol on it.
   *
   * Shared by the connection itself and by `cancel()`, which has to reach the
   * same server the same way — a cancel that skipped TLS against a server
   * requiring it would simply be refused.
   */
  async #dial(options: PgOptions): Promise<{ socket: PgSocket; frames: FrameReader | null }> {
    const host = options.host ?? "localhost";
    const port = options.port ?? 5432;
    const sslmode = options.sslmode ?? "prefer";
    const wantsTls = sslmode !== "disable";

    const tlsOptions = options.sslRootCert === undefined ? {} : { ca: options.sslRootCert };
    let socket = netConnect(
      { hostname: host, port },
      wantsTls ? { secureTransport: "starttls", ...tlsOptions } : {},
    );
    await socket.opened;
    let frames: FrameReader | null = null;

    if (wantsTls) {
      // libpq's dance, and the reason `runtime:net` has `startTls()` at all:
      // the connection opens in plaintext, asks, and only then becomes TLS.
      const writer = socket.writable.getWriter();
      await writer.write(msg.sslRequest());
      writer.releaseLock();
      // Reading the answer takes a reader, and a reader **locks the stream**.
      // So the probe is kept rather than replaced: if the server declines, this
      // is the reader for the rest of the connection. Building a second one on
      // the same socket would throw on a stream that is already locked — which
      // is the default path, against any server without TLS configured.
      const probe = new FrameReader(socket.readable);
      const answer = await probe.byte();
      if (answer === 0x53) {
        // 'S' — the server agreed. The probe goes with the plaintext socket,
        // which `startTls()` consumes; the upgraded one has streams of its own.
        socket = socket.startTls();
        await socket.opened;
      } else if (sslmode === "require") {
        throw new DbError(
          `the server refused TLS and sslmode is 'require' (it answered ${JSON.stringify(String.fromCharCode(answer))})`,
          { code: DbErrorCode.Unsupported },
        );
      } else {
        frames = probe;
      }
    }

    return { socket, frames };
  }

  /**
   * Asks the server to cancel whatever this connection is running.
   *
   * On a connection of its own, because the protocol leaves no choice: the one
   * running the query is busy reading the answer, which is the thing being
   * cancelled. The server replies to nothing and closes — so this returns once
   * the request has been *sent*, not once anything has been cancelled, and the
   * outcome shows up at the query: a `57014` error, or nothing at all if it had
   * already finished. Cancellation is a request rather than an instruction, and
   * the protocol is honest about that.
   */
  async cancel(): Promise<void> {
    if (this.#processId === 0) return;
    const { socket } = await this.#dial(this.#target);
    try {
      const writer = socket.writable.getWriter();
      await writer.write(msg.cancelRequest(this.#processId, this.#secretKey));
      writer.releaseLock();
    } finally {
      await socket.close().catch(() => {});
    }
  }

  /** Latches the first transport failure and tears the connection down. */
  #die(cause: unknown): DbError {
    if (this.#fatal === null) {
      const detail = cause instanceof Error ? cause.message : String(cause);
      this.#fatal = new DbError(`the connection to the server was lost: ${detail}`, {
        code: DbErrorCode.ConnectionLost,
        cause,
      });
      // Nothing is streaming any more, whatever a result set believes, and the
      // socket goes with it: a half-read connection is not a resource worth
      // holding on to.
      this.#streaming = false;
      void this.#teardown();
    }
    return this.#fatal;
  }

  async #send(bytes: Uint8Array): Promise<void> {
    if (this.#fatal !== null) throw this.#fatal;
    const writer = this.#writer;
    if (writer === null) throw new DbError("the connection is closed", { code: DbErrorCode.Closed });
    try {
      await writer.write(bytes);
    } catch (e) {
      throw this.#die(e);
    }
  }

  async #next(): Promise<{ tag: number; frame: Uint8Array }> {
    if (this.#fatal !== null) throw this.#fatal;
    const frames = this.#frames;
    if (frames === null) throw new DbError("the connection is closed", { code: DbErrorCode.Closed });
    try {
      return await frames.message();
    } catch (e) {
      throw this.#die(e);
    }
  }

  /**
   * Takes the connection for one exchange, returning the release.
   *
   * Every caller must release in a `finally`: a holder that throws without
   * releasing would leave the chain waiting on a promise nobody settles, and
   * the connection would be lost rather than merely broken.
   */
  async #acquire(): Promise<() => void> {
    // A dead connection answers immediately rather than queueing behind an
    // exchange that will never finish.
    if (this.#fatal !== null) throw this.#fatal;
    if (this.#pump !== null) {
      throw new DbError(
        "this connection is listening for notifications and runs no queries — use another connection, or a pool",
        { code: DbErrorCode.ConnectionBusy },
      );
    }
    if (this.#streaming) {
      throw new DbError(
        "this connection is streaming a result set — finish it (await rows.toArray(), or let the for-await end), or run the second query on another connection",
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
    return release;
  }

  async #authenticate(options: PgOptions): Promise<void> {
    let session: Awaited<ReturnType<ReturnType<typeof scram>["final"]>> | null = null;
    for (;;) {
      const { tag, frame } = await this.#next();
      const fields = new Fields(frame);
      switch (tag) {
        case B.Authentication: {
          const kind = fields.i32();
          if (kind === AUTH.Ok) break;
          if (kind === AUTH.CleartextPassword) {
            await this.#send(msg.password(this.#requirePassword(options)));
            break;
          }
          if (kind === AUTH.SASL) {
            const mechanisms: string[] = [];
            for (;;) {
              const name = fields.cstring();
              if (name === "") break;
              mechanisms.push(name);
            }
            if (!mechanisms.includes("SCRAM-SHA-256")) {
              throw new DbError(
                `the server offered ${mechanisms.join(", ") || "no"} authentication; this driver speaks SCRAM-SHA-256`,
                { code: DbErrorCode.AuthFailed },
              );
            }
            const exchange = scram(this.#requirePassword(options));
            this.#scram = exchange;
            await this.#send(msg.saslInitialResponse("SCRAM-SHA-256", exchange.initial));
            break;
          }
          if (kind === AUTH.SASLContinue) {
            const serverFirst = new TextDecoder().decode(frame.subarray(fields.at));
            session = await this.#scram!.final(serverFirst);
            await this.#send(msg.saslResponse(session.message));
            break;
          }
          if (kind === AUTH.SASLFinal) {
            const serverFinal = new TextDecoder().decode(frame.subarray(fields.at));
            // Mutual authentication. Skipping this would mean the client proved
            // itself to whoever answered and learned nothing in return.
            session!.verify(serverFinal);
            break;
          }
          if (kind === AUTH.MD5Password) {
            throw new DbError(
              "the server asked for md5 authentication, which this driver does not implement — configure the server for scram-sha-256 (the default since PostgreSQL 14)",
              { code: DbErrorCode.AuthFailed },
            );
          }
          throw new DbError(`unsupported authentication request ${kind}`, {
            code: DbErrorCode.AuthFailed,
          });
        }
        case B.BackendKeyData:
          this.#processId = fields.i32();
          this.#secretKey = fields.i32();
          break;
        case B.ErrorResponse:
          throw serverError(readServerMessage(fields));
        case B.ReadyForQuery:
          this.status = String.fromCharCode(fields.u8());
          return;
        default:
          this.#observe(tag, fields);
          break;
      }
    }
  }

  /**
   * The failure that ended this connection, latched.
   *
   * A transport error is not one operation's problem: once a message has been
   * half-read off a socket, nothing later on that socket can be trusted to
   * start on a boundary. So the first one is kept and every later call is
   * answered with it, rather than each caller discovering a different symptom
   * of the same dead connection — a hang, a length that makes no sense, a
   * message tag nobody sent.
   */
  #fatal: DbError | null = null;

  #scram: ReturnType<typeof scram> | null = null;
  #processId = 0;
  #secretKey = 0;

  #requirePassword(options: PgOptions): string {
    if (options.password === undefined) {
      throw new DbError("the server asked for a password and none was given", {
        code: DbErrorCode.AuthFailed,
      });
    }
    return options.password;
  }

  // -- queries --------------------------------------------------------------

  /**
   * SQL text → the name it is prepared under.
   *
   * Insertion-ordered, which is what makes eviction a `keys().next()` rather
   * than a sort: the oldest entry is the first key, and a hit re-inserts to
   * move itself to the back.
   */
  #statements = new Map<string, Prepared>();
  #nextStatement = 0;
  #cacheLimit = 100;

  /**
   * Prepares `text`, learning its shape once.
   *
   * The shape has to be known *before* `Bind`, because `Bind` is what carries
   * the result formats — and the server only reports columns after it. So a
   * statement-level `Describe` runs with the `Parse`, one extra round trip the
   * first time a statement is seen and none of the times after, which is
   * exactly what the statement cache already made cheap.
   */
  async #prepare(text: string): Promise<Prepared> {
    const cached = this.#statements.get(text);
    if (cached !== undefined) {
      // Re-inserting moves it to the back, making eviction least-recently-used.
      this.#statements.delete(text);
      this.#statements.set(text, cached);
      return cached;
    }

    const parts: Uint8Array[] = [];
    while (this.#statements.size >= this.#cacheLimit) {
      const oldest = this.#statements.keys().next();
      if (oldest.done === true) break;
      parts.push(msg.closeStatement(this.#statements.get(oldest.value)!.name));
      this.#statements.delete(oldest.value);
    }
    const name = `esrun_s${this.#nextStatement++}`;
    parts.push(msg.parse(name, text), msg.describeStatement(name), msg.sync());
    await this.#send(msg.concat(parts));

    const columns = await this.#readShape();
    // Binary where it is simpler and cheaper to read than text, and text where
    // it is not. An all-text statement sends no format list at all, which is
    // both shorter and exactly what the server assumes.
    const formats = columns === null ? [] : columns.oids.map((oid) => (prefersBinary(oid) ? 1 : 0));
    const entry: Prepared = {
      name,
      columns,
      formats: formats.includes(1) ? formats : [],
    };
    this.#statements.set(text, entry);
    return entry;
  }

  /** Reads the answer to a statement-level `Describe`, up to `ReadyForQuery`. */
  async #readShape(): Promise<Columns | null> {
    let columns: Columns | null = null;
    let failure: unknown = null;
    for (;;) {
      const { tag, frame } = await this.#next();
      const fields = new Fields(frame);
      switch (tag) {
        case B.RowDescription:
          columns = readRowDescription(fields);
          break;
        case B.NoData:
          columns = null;
          break;
        case B.ErrorResponse:
          failure = serverError(readServerMessage(fields));
          break;
        case B.ReadyForQuery:
          this.status = String.fromCharCode(fields.u8());
          if (failure !== null) throw failure;
          return columns;
        default:
          this.#observe(tag, fields);
          break;
      }
    }
  }

  /**
   * Sends the parameters and starts the statement, returning its shape.
   *
   * With caching off there is nothing to amortize a describe against, so this
   * keeps the older shape-per-execution path: everything in one write, all
   * columns in text.
   */
  async #start(text: string, params: unknown[]): Promise<Columns | null> {
    const bound = params.map((value) => encodeParam(value));
    if (this.#cacheLimit === 0) {
      await this.#send(
        msg.concat([
          msg.parse("", text),
          msg.bind("", "", bound),
          msg.describePortal(""),
          msg.execute("", 0),
          msg.sync(),
        ]),
      );
      return this.#describe();
    }

    const prepared = await this.#prepare(text);
    await this.#send(
      msg.concat([
        msg.bind("", prepared.name, bound, prepared.formats),
        msg.execute("", 0),
        msg.sync(),
      ]),
    );
    await this.#awaitBindComplete();
    return prepared.columns;
  }

  /** Reads up to `BindComplete`, leaving the rows for the batch reader. */
  async #awaitBindComplete(): Promise<void> {
    for (;;) {
      const { tag, frame } = await this.#next();
      if (tag === B.BindComplete) return;
      const fields = new Fields(frame);
      if (tag === B.ErrorResponse) {
        const error = readServerMessage(fields);
        await this.#drainToReady();
        throw serverError(error);
      }
      this.#observe(tag, fields);
    }
  }

  /**
   * Whether `error` says a cached plan has gone stale and the statement should
   * be prepared again.
   *
   * Two ways it happens, neither the caller's doing. The table changed under a
   * plan, so the server refuses to reuse it (`0A000`, "cached plan must not
   * change result type"). Or the statement is simply not there (`26000`) — a
   * pooler reset the session, or something ran `DISCARD ALL`. A cache that
   * turned either into an application error would be worse than no cache.
   */
  #isStalePlan(error: unknown): boolean {
    const code = (error as { backendCode?: string } | null)?.backendCode;
    return code === "0A000" || code === "26000" || code === "42P05";
  }

  /**
   * Reads until the statement's rows begin, returning the column description —
   * or `null` when the statement returns no rows at all.
   *
   * Only the uncached path uses this: with a statement cache the shape is
   * already known, and asking again per execution would be a message and a
   * parse for an answer that cannot have changed.
   */
  async #describe(): Promise<Columns | null> {
    for (;;) {
      const { tag, frame } = await this.#next();
      const fields = new Fields(frame);
      switch (tag) {
        case B.ParseComplete:
        case B.BindComplete:
          break;
        case B.RowDescription:
          return readRowDescription(fields);
        case B.NoData:
          return null;
        case B.ErrorResponse: {
          const error = readServerMessage(fields);
          await this.#drainToReady();
          throw serverError(error);
        }
        default:
          this.#observe(tag, fields);
          break;
      }
    }
  }

  /** Reads rows until the batch is full or the statement finishes. */
  async #batch(columns: number): Promise<Batch> {
    const frames: Uint8Array[] = [];
    let size = 0;
    let rows = 0;
    for (;;) {
      const { tag, frame } = await this.#next();
      if (tag === B.DataRow) {
        // A `DataRow` frame *is* the shared row encoding — length, column
        // count, then each column's length and bytes. So it is copied into the
        // batch as-is and never transcoded. The copy is required: the frame is
        // a view into the read buffer, which the next message overwrites.
        frames.push(frame.slice());
        size += frame.length;
        rows++;
        if (size >= BATCH_BYTES) {
          return { bytes: join(frames, size), rows, done: false };
        }
        continue;
      }
      const fields = new Fields(frame);
      switch (tag) {
        case B.CommandComplete:
        case B.EmptyQueryResponse:
          this.#lastTag = tag === B.CommandComplete ? fields.cstring() : "";
          await this.#drainToReady();
          return { bytes: join(frames, size), rows, done: true };
        case B.ErrorResponse: {
          const error = readServerMessage(fields);
          await this.#drainToReady();
          throw serverError(error);
        }
        case B.PortalSuspended:
          return { bytes: join(frames, size), rows, done: false };
        default:
          this.#observe(tag, fields);
          break;
      }
      void columns;
    }
  }

  #lastTag = "";

  /** Reads and discards until `ReadyForQuery`, so the connection is reusable. */
  async #drainToReady(): Promise<void> {
    for (;;) {
      const { tag, frame } = await this.#next();
      if (tag === B.ReadyForQuery) {
        this.status = String.fromCharCode(new Fields(frame).u8());
        return;
      }
      if (tag === B.ErrorResponse) {
        // An error while draining is recorded, not thrown: the caller is
        // already unwinding from the first one.
        continue;
      }
      this.#observe(tag, new Fields(frame));
    }
  }

  /**
   * Runs the send-and-describe half of an exchange, preparing again if the
   * server says the cached plan is stale.
   *
   * Retried exactly once, and only for that: an error response is followed by a
   * drain to `ReadyForQuery`, so the connection is clean and the second attempt
   * starts from the same place the first did. A second failure is the caller's
   * to see.
   */
  async #askAndDescribe(text: string, params: unknown[]): Promise<Columns | null> {
    try {
      return await this.#start(text, params);
    } catch (e) {
      if (!this.#isStalePlan(e)) throw e;
      // The whole cache goes, not just this entry: whatever invalidated one
      // plan — a migration, a DISCARD — rarely stopped at one.
      this.#statements.clear();
      return await this.#start(text, params);
    }
  }

  protected async _query(q: {
    text: string;
    positional: unknown[];
    named: [string, unknown][];
  }): Promise<Rows> {
    this.#rejectNamed(q.named);
    const release = await this.#acquire();
    let held = true;
    // Released exactly once, however this ends: an early return, a throw, or
    // the result set finishing much later.
    const releaseOnce = () => {
      if (!held) return;
      held = false;
      this.#streaming = false;
      release();
    };

    try {
      const described = await this.#askAndDescribe(q.text, q.positional);
      if (described === null) {
        // A statement with no result — `INSERT` without `RETURNING`, run
        // through `query()`. Finish it and hand back an empty result rather
        // than leaving the connection mid-exchange.
        await this.#batch(0);
        releaseOnce();
        return new Rows(emptySource(), defineRowShape([]));
      }
      const columns = described.names.map((name, i) => ({
        name,
        declType: null,
        // The type is fixed for the whole column, so the decoder is chosen once
        // here rather than per value — which is what the shared row format
        // makes possible, and what lets a column be asked for in binary at all.
        oid: described.oids[i]!,
      }));
      const formats = this.#statements.get(q.text)?.formats ?? [];
      const shape = defineRowShape(columns, {
        decoders: described.oids.map((oid, i) => decoderForFormat(oid, formats[i] ?? 0)),
      });

      let first: Batch | null = await this.#batch(columns.length);
      if (first.done) {
        // The whole result arrived in one batch, so the exchange is over and
        // the connection is free before the caller has read a single row.
        releaseOnce();
        return new Rows(oneBatch(first), shape);
      }

      // More to come: the lock stays held, and the result set owns it.
      this.#streaming = true;
      const self = this;
      return new Rows(
        {
          exhausted: false,
          async next(): Promise<Batch> {
            if (first !== null) {
              const batch = first;
              first = null;
              return batch;
            }
            try {
              const batch = await self.#batch(columns.length);
              if (batch.done) releaseOnce();
              return batch;
            } catch (e) {
              releaseOnce();
              throw e;
            }
          },
          async close(): Promise<void> {
            if (!held) return;
            // A caller that stopped early left the server mid-result. The rest
            // has to come off the wire before anything else can be asked, or
            // the next query would read this one's rows. This is what
            // `release(clean)` will assert once there is a pool.
            try {
              for (;;) {
                const batch = await self.#batch(columns.length);
                if (batch.done) return;
              }
            } finally {
              releaseOnce();
            }
          },
        },
        shape,
      );
    } catch (e) {
      releaseOnce();
      throw e;
    }
  }

  protected async _execute(q: {
    text: string;
    positional: unknown[];
    named: [string, unknown][];
  }): Promise<{ changes: number; lastInsertRowid: number | null }> {
    this.#rejectNamed(q.named);
    const release = await this.#acquire();
    try {
      await this.#askAndDescribe(q.text, q.positional);
      await this.#batch(0);
      return { changes: affectedRows(this.#lastTag), lastInsertRowid: null };
    } finally {
      release();
    }
  }

  /**
   * Runs `work` with an `AbortSignal` attached.
   *
   * Aborting sends a cancel and then waits: the server answers the *query* with
   * `57014`, and only then is the connection back in a known state. Rejecting
   * the caller the instant the signal fired would leave a statement running and
   * a connection mid-exchange, which is worse than waiting a moment for the
   * cancellation to land.
   *
   * What the caller sees is their own `reason`, not the server's error. They
   * asked for the abort; `57014` is a detail of how the asking was carried out.
   */
  async #withSignal<T>(signal: AbortSignal | undefined, work: () => Promise<T>): Promise<T> {
    if (signal === undefined) return work();
    if (signal.aborted) throw signal.reason;
    const onAbort = (): void => {
      void this.cancel().catch(() => {});
    };
    signal.addEventListener("abort", onAbort, { once: true });
    try {
      return await work();
    } catch (e) {
      if (signal.aborted) throw signal.reason;
      throw e;
    } finally {
      signal.removeEventListener("abort", onAbort);
    }
  }

  /**
   * As `Connection.query`, plus an `AbortSignal`.
   *
   * The signal stays attached until the **rows** end, not merely until the
   * first batch arrives: a streaming result is still the query running, and a
   * caller who abandons one halfway is exactly who wanted to cancel.
   */
  override async query(
    q: Parameters<BaseConnection["query"]>[0],
    params?: Parameters<BaseConnection["query"]>[1],
    options: { signal?: AbortSignal } = {},
  ): Promise<Rows> {
    const signal = options.signal;
    if (signal === undefined) return super.query(q, params);
    if (signal.aborted) throw signal.reason;

    const onAbort = (): void => {
      void this.cancel().catch(() => {});
    };
    signal.addEventListener("abort", onAbort, { once: true });
    let rows: Rows;
    try {
      rows = await super.query(q, params);
    } catch (e) {
      signal.removeEventListener("abort", onAbort);
      if (signal.aborted) throw signal.reason;
      throw e;
    }
    if (rows.exhausted) {
      // Already complete, so there is nothing left to cancel and nothing to
      // keep listening for.
      signal.removeEventListener("abort", onAbort);
      return rows;
    }

    const close = rows.close.bind(rows);
    rows.close = async () => {
      try {
        await close();
      } finally {
        signal.removeEventListener("abort", onAbort);
      }
    };

    // The failure from an aborted stream arrives *out of the iterator*, not out
    // of the call that started it — so without this the caller would get the
    // server's `57014` here and their own reason from `execute()`, for the same
    // act. `toArray()` and `first()` iterate through this too.
    const iterate = rows[Symbol.asyncIterator].bind(rows);
    rows[Symbol.asyncIterator] = async function* wrapped() {
      const inner = iterate();
      try {
        for (;;) {
          const next = await inner.next();
          if (next.done === true) return;
          yield next.value;
        }
      } catch (e) {
        if (signal.aborted) throw signal.reason;
        throw e;
      } finally {
        // Forwarded, not assumed: a caller that breaks out of *this* generator
        // must still run the inner one's cleanup, which is what closes the
        // cursor and gives the connection back.
        await inner.return?.(undefined);
      }
    };
    return rows;
  }

  override async execute(
    q: Parameters<BaseConnection["execute"]>[0],
    params?: Parameters<BaseConnection["execute"]>[1],
    options: { signal?: AbortSignal } = {},
  ): Promise<{ changes: number; lastInsertRowid: number | null }> {
    return this.#withSignal(options.signal, () => super.execute(q, params));
  }

  /**
   * Runs a script — several statements in one string — through the simple query
   * protocol.
   *
   * `query()` and `execute()` use the extended protocol, which prepares the
   * statement, and a prepared statement is one statement by definition: a
   * string with two of them is refused by the server with "cannot insert
   * multiple commands into a prepared statement". That is the right answer for
   * a query and the wrong one for a migration, so scripts get their own door.
   *
   * Two consequences worth knowing. **No parameters** — the simple protocol has
   * nowhere to put them, so anything variable has to be quoted into the text,
   * and quoting values into SQL is how injection happens. Use it for schema and
   * fixed statements, not for data from outside. And PostgreSQL wraps a
   * multi-statement string in a **single implicit transaction**, so a failure
   * part-way rolls back everything before it — unless the script manages its own
   * transactions, in which case it gets what it asked for.
   *
   * Rows are discarded: this reports what each statement did, not what it
   * returned.
   */
  async executeScript(
    sql: string,
    options: { signal?: AbortSignal } = {},
  ): Promise<{ command: string; changes: number }[]> {
    this._open();
    return this.#withSignal(options.signal, () => this.#runScript(sql));
  }

  async #runScript(sql: string): Promise<{ command: string; changes: number }[]> {
    const release = await this.#acquire();
    try {
      // A script is where DDL and DISCARD live, and both can invalidate plans
      // the cache is holding. Forgetting them costs a re-parse each; keeping a
      // stale one costs an error the caller did not cause.
      this.#statements.clear();
      await this.#send(msg.simpleQuery(sql));
      const results: { command: string; changes: number }[] = [];
      for (;;) {
        const { tag, frame } = await this.#next();
        if (tag === B.DataRow) continue; // a script reports, it does not return
        const fields = new Fields(frame);
        switch (tag) {
          case B.CommandComplete: {
            const completion = fields.cstring();
            results.push({
              command: completion.split(" ")[0] ?? completion,
              changes: affectedRows(completion),
            });
            break;
          }
          case B.ErrorResponse: {
            const error = readServerMessage(fields);
            await this.#drainToReady();
            throw serverError(error);
          }
          case B.ReadyForQuery:
            this.status = String.fromCharCode(fields.u8());
            return results;
          default:
            this.#observe(tag, fields);
            break;
        }
      }
    } finally {
      release();
    }
  }

  // -- LISTEN/NOTIFY --------------------------------------------------------

  /**
   * Called for each `NOTIFY` on a channel this connection is listening to.
   *
   * `payload` is `""` when the notifier sent none, and `processId` is the
   * backend that sent it — which is how a connection recognises its own
   * notifications, since PostgreSQL delivers them to the sender too.
   */
  onNotification:
    | ((notification: { channel: string; payload: string; processId: number }) => void)
    | undefined;

  /** Called when the listening loop fails, since nobody is awaiting it. */
  onListenError: ((error: unknown) => void) | undefined;

  #pump: Promise<void> | null = null;
  /** Commands sent into the pump, awaiting their `ReadyForQuery`. */
  #pumpCommands: { resolve: () => void; reject: (e: unknown) => void }[] = [];
  #channels = new Set<string>();

  /** The channels this connection is listening to. */
  get channels(): string[] {
    return [...this.#channels];
  }

  /** Whether this connection has been given over to listening. */
  get listening(): boolean {
    return this.#pump !== null;
  }

  /**
   * Starts listening on `channel`.
   *
   * A notification arrives when it arrives, and a connection only sees messages
   * while it is reading — which an idle one is not. So the first `listen()`
   * gives this connection over to a **read loop**, and from then on it runs no
   * queries: `query()` and `execute()` refuse with `ERR_DB_CONNECTION_BUSY`.
   * That is not a limitation worked around but how you would deploy it anyway —
   * a connection that must notice a notification promptly should not be waiting
   * behind someone's report query.
   *
   * The loop owns *reading*; a `LISTEN` only needs *writing*, and TCP is full
   * duplex — so commands go out underneath the loop and it resolves them when
   * their `ReadyForQuery` comes back. That is why this can await confirmation
   * rather than hope: a misspelled channel fails here, instead of silently
   * never firing.
   */
  async listen(channel: string): Promise<void> {
    this._open();
    this.#startPump();
    await this.#pumpCommand(`LISTEN ${POSTGRES_DIALECT.quoteIdent(channel)}`);
    this.#channels.add(channel);
  }

  /** Stops listening on `channel`. The read loop stays, and so do the others. */
  async unlisten(channel: string): Promise<void> {
    this._open();
    if (this.#pump === null) return;
    await this.#pumpCommand(`UNLISTEN ${POSTGRES_DIALECT.quoteIdent(channel)}`);
    this.#channels.delete(channel);
  }

  #pumpCommand(sql: string): Promise<void> {
    return new Promise<void>((resolve, reject) => {
      this.#pumpCommands.push({ resolve, reject });
      this.#send(msg.simpleQuery(sql)).catch(reject);
    });
  }

  #settle(error?: unknown): void {
    const pending = this.#pumpCommands.shift();
    if (pending === undefined) return;
    if (error === undefined) pending.resolve();
    else pending.reject(error);
  }

  #startPump(): void {
    if (this.#pump !== null) return;
    this.#pump = (async () => {
      for (;;) {
        const { tag, frame } = await this.#next();
        const fields = new Fields(frame);
        switch (tag) {
          case B.NotificationResponse: {
            const processId = fields.i32();
            const channel = fields.cstring();
            const payload = fields.cstring();
            this.onNotification?.({ channel, payload, processId });
            break;
          }
          case B.ReadyForQuery:
            this.status = String.fromCharCode(fields.u8());
            this.#settle();
            break;
          case B.ErrorResponse:
            this.#settle(serverError(readServerMessage(fields)));
            break;
          case B.CommandComplete:
            break;
          default:
            this.#observe(tag, fields);
            break;
        }
      }
    })().catch((e) => {
      // The loop only ends when the connection does. Everyone still waiting on
      // a command hears about it, and the failure goes to a handler rather than
      // becoming an unhandled rejection nobody asked for.
      for (const pending of this.#pumpCommands.splice(0)) pending.reject(e);
      this.onListenError?.(e);
    });
  }

  /**
   * Messages that can arrive at any point and are nobody's answer.
   *
   * PostgreSQL may send these between any two messages of an exchange, so every
   * read loop routes what it does not recognise here rather than dropping it.
   * Ignoring them is what made `parameters` go stale after `SET TIME ZONE` and
   * made `RAISE NOTICE` output vanish.
   */
  #observe(tag: number, fields: Fields): void {
    switch (tag) {
      case B.ParameterStatus: {
        // The server reports a GUC it thinks the client should track — the time
        // zone, the encoding, `search_path`. It sends these unprompted whenever
        // one changes, which is why they cannot only be read at the handshake.
        this.parameters[fields.cstring()] = fields.cstring();
        break;
      }
      case B.NoticeResponse: {
        const notice = readServerMessage(fields);
        // Same shape as an error and deliberately not thrown: a notice is the
        // server talking, not the statement failing. Unhandled, it is dropped
        // rather than logged — a driver that printed to stderr on its own would
        // be a driver you had to work around.
        this.onNotice?.(notice);
        break;
      }
      default:
        break;
    }
  }

  /**
   * Called for each `NOTICE`/`WARNING` the server sends — `RAISE NOTICE` in a
   * function, a deprecation, a truncation. Unset, they are discarded.
   */
  onNotice: ((notice: ServerMessage) => void) | undefined;

  #rejectNamed(named: [string, unknown][]): void {
    if (named.length > 0) {
      throw new DbError(
        "PostgreSQL binds parameters by position; pass an array and use $1, $2, … (or the sql`` tag)",
        { code: DbErrorCode.Unsupported },
      );
    }
  }

  protected async _close(): Promise<void> {
    // A polite goodbye only makes sense on a connection that is still there.
    if (this.#fatal === null && this.#writer !== null) {
      try {
        await this.#send(msg.terminate());
      } catch {
        /* the peer may already be gone; the teardown below is what matters */
      }
    }
    await this.#teardown();
  }

  async #teardown(): Promise<void> {
    const [socket, frames, writer] = [this.#socket, this.#frames, this.#writer];
    this.#socket = null;
    this.#frames = null;
    this.#writer = null;
    try {
      writer?.releaseLock();
      await frames?.cancel();
      await socket?.close();
    } catch {
      /* closing twice is not an error */
    }
  }
}

function join(frames: Uint8Array[], size: number): Uint8Array {
  const out = new Uint8Array(size);
  let at = 0;
  for (const frame of frames) {
    out.set(frame, at);
    at += frame.length;
  }
  return out;
}

const NOTHING: Batch = { bytes: new Uint8Array(0), rows: 0, done: true };

/** A finished result: one batch to hand over, then nothing, and no cursor. */
function oneBatch(batch: Batch) {
  let pending: Batch | null = batch;
  return {
    exhausted: true,
    async next(): Promise<Batch> {
      const value = pending ?? NOTHING;
      pending = null;
      return value;
    },
    async close(): Promise<void> {},
  };
}

/** A statement that returned no result set at all. */
function emptySource() {
  return oneBatch(NOTHING);
}

/** `INSERT 0 3` / `UPDATE 2` / `SELECT 7` — the count is the last word. */
function affectedRows(tag: string): number {
  const parts = tag.trim().split(" ");
  const last = parts[parts.length - 1];
  const count = last === undefined ? Number.NaN : Number(last);
  return Number.isFinite(count) ? count : 0;
}

function readServerMessage(fields: Fields): ServerMessage {
  const out: Record<string, string> = {};
  for (;;) {
    if (fields.done) break;
    const kind = fields.u8();
    if (kind === 0) break;
    out[String.fromCharCode(kind)] = fields.cstring();
  }
  return {
    severity: out["S"] ?? out["V"] ?? "ERROR",
    code: out["C"] ?? "",
    message: out["M"] ?? "the server reported an error with no message",
    ...(out["D"] === undefined ? {} : { detail: out["D"] }),
    ...(out["H"] === undefined ? {} : { hint: out["H"] }),
    ...(out["P"] === undefined ? {} : { position: out["P"] }),
    ...(out["s"] === undefined ? {} : { schema: out["s"] }),
    ...(out["t"] === undefined ? {} : { table: out["t"] }),
    ...(out["c"] === undefined ? {} : { column: out["c"] }),
    ...(out["n"] === undefined ? {} : { constraint: out["n"] }),
  };
}

function serverError(server: ServerMessage): DbError {
  const error = asDbError(
    Object.assign(new Error(server.message), { code: server.code }),
    portableCode(server.code),
  );
  // Everything the server said, kept: an ORM wants the constraint name, and a
  // human wants the hint.
  return Object.assign(error, { server });
}
