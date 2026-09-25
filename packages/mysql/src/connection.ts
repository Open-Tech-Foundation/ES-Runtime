/**
 * A MySQL connection: the client/server protocol, in JavaScript, over
 * `runtime:net`.
 *
 * There is no native code anywhere in this package. The transport is a socket
 * and a TLS upgrade; everything above it — framing, authentication, prepared
 * statements, row decoding — is here, which is the arrangement `runtime:db`
 * exists to make possible (DECISIONS D56).
 */

import {
  asDbError,
  BaseConnection,
  DbError,
  DbErrorCode,
  type DbOutput,
  Dialect,
  defineRowShape,
  type NormalizedQuery,
  type Row,
  Rows,
} from "runtime:db";
import { connect as netConnect } from "runtime:net";

import { cachingSha2, encryptPassword, nativePassword, passwordBytes } from "./protocol/auth.js";
import { portableCode, type ServerError } from "./protocol/errors.js";
import { PacketReader, Payload, Writer } from "./protocol/packets.js";
import {
  type Column,
  type DecodeOptions,
  decoderFor,
  RowBatch,
  readColumn,
  shapeKey,
  T,
  widths,
  writeParams,
} from "./protocol/values.js";

/** What `runtime:net`'s `connect()` hands back. */
type MySqlSocket = ReturnType<typeof netConnect>;

// Capability flags (`CLIENT_*`) this driver asks for.
const CLIENT = {
  LONG_PASSWORD: 0x1,
  FOUND_ROWS: 0x2,
  LONG_FLAG: 0x4,
  CONNECT_WITH_DB: 0x8,
  PROTOCOL_41: 0x200,
  SSL: 0x800,
  TRANSACTIONS: 0x2000,
  SECURE_CONNECTION: 0x8000,
  MULTI_STATEMENTS: 0x1_0000,
  MULTI_RESULTS: 0x2_0000,
  PS_MULTI_RESULTS: 0x4_0000,
  PLUGIN_AUTH: 0x8_0000,
  PLUGIN_AUTH_LENENC_CLIENT_DATA: 0x20_0000,
  DEPRECATE_EOF: 0x100_0000,
} as const;

// Server status flags.
const STATUS_IN_TRANS = 0x1;
const STATUS_MORE_RESULTS = 0x8;

// Commands.
const COM = {
  QUIT: 0x01,
  QUERY: 0x03,
  PING: 0x0e,
  STMT_PREPARE: 0x16,
  STMT_EXECUTE: 0x17,
  STMT_CLOSE: 0x19,
} as const;

/** `ER_UNSUPPORTED_PS`: this statement cannot be prepared — run it as text. */
const ER_UNSUPPORTED_PS = 1295;

/** How much of a result set to gather before handing it to the caller. */
const BATCH_BYTES = 64 * 1024;

export const MYSQL_DIALECT: Dialect = new Dialect({
  name: "mysql",
  placeholder: () => "?",
  quote: "`",
  supports: {
    // MariaDB has `RETURNING` for INSERT and DELETE; MySQL has none. A dialect
    // is one answer for both, so it is the answer that holds for both.
    returning: false,
    savepoints: true,
    // The protocol binds by position. `:name` would mean rewriting SQL here,
    // which means parsing SQL here, which a driver should not do.
    namedParameters: false,
  },
});

export interface MySqlOptions {
  host?: string;
  port?: number;
  user?: string;
  password?: string;
  database?: string;
  /**
   * `"prefer"` (default) uses TLS when the server offers it; `"require"`
   * insists; `"disable"` never asks.
   *
   * TLS here is **verified**, as it is everywhere in this runtime. A stock
   * MySQL server offers TLS with a self-signed certificate it generated for
   * itself, which nothing can verify — so against one, `"prefer"` fails with
   * an error saying so, rather than encrypting to a server it cannot identify.
   * Give it the server's authority with `sslRootCert`, or say `"disable"`.
   */
  sslmode?: "require" | "prefer" | "disable";
  /** A certificate authority to trust in addition to the public roots, as PEM. */
  sslRootCert?: string | Uint8Array;
  /**
   * How long to wait for the connection **and its handshake**, in
   * milliseconds. Default 10 000; `0` waits forever.
   */
  connectTimeout?: number;
  /**
   * A per-statement time limit in milliseconds, enforced by the **server**.
   * MySQL's `max_execution_time` applies it to `SELECT` only; MariaDB's
   * `max_statement_time` applies it to everything. Default unset (no limit).
   */
  statementTimeout?: number;
  /**
   * How many prepared statements to keep per connection. Default 100.
   *
   * Each is a plan the server holds, bounded by its `max_prepared_stmt_count`
   * across every connection — so the bound matters as much as the cache.
   */
  preparedStatementCacheSize?: number;
  /**
   * Decode date and time columns to **Temporal** values. Default `true`.
   * `false` gives `Date` for `DATETIME` and `TIMESTAMP` and strings for
   * `DATE` and `TIME`.
   */
  temporal?: boolean;
  /**
   * The server's RSA public key, as PEM, for `caching_sha2_password` over a
   * connection without TLS.
   *
   * When that plugin needs the password itself — the first login after the
   * server restarts — a plaintext connection has to encrypt it to the server's
   * key. Naming the key here is the safe way: the password goes to the server
   * that holds it and to nobody else.
   */
  serverPublicKey?: string;
  /**
   * Ask the server for its public key when `serverPublicKey` is not given.
   * Default `false`.
   *
   * The key arrives over the same unauthenticated connection it protects, so
   * anyone able to answer in the server's place can send their own and read
   * the password. That is a trade to make knowingly — on a network you trust —
   * and so it is opt-in, as it is in MySQL's own Connector/J. The URL spells it
   * `allowPublicKeyRetrieval=true`.
   */
  allowPublicKeyRetrieval?: boolean;
}

/**
 * What a MySQL column can produce: numbers, bigints, strings, bytes, a parsed
 * JSON document, or a Temporal value.
 */
export type MySqlValue = DbOutput | Date | object;

/** A row from this backend. */
export type MySqlRow = Row<MySqlValue>;

interface Batch {
  bytes: Uint8Array;
  rows: number;
  done: boolean;
}

interface Prepared {
  id: number;
  params: number;
  /** Not cached: closed once it has run, because nothing will run it again. */
  transient: boolean;
}

/** The fields of an `OK` packet that anyone asks about. */
interface Ok {
  affectedRows: number;
  lastInsertId: number;
  status: number;
}

/** The head of a response: a result set's columns, or a statement's `OK`. */
type Head = { columns: Column[] } | { ok: Ok };

export class MySqlConnection extends BaseConnection {
  #socket: MySqlSocket | null = null;
  #reader: PacketReader | null = null;
  #writer: WritableStreamDefaultWriter<Uint8Array> | null = null;
  #capabilities = 0;
  /** The server's version string, from its greeting. */
  serverVersion = "";
  /** This connection's id on the server — what `KILL QUERY` names. */
  connectionId = 0;
  /** The server status flags from the last `OK` or `EOF`. */
  #status = 0;
  #fatal: DbError | null = null;
  #target: MySqlOptions = {};
  #decode: DecodeOptions = { temporal: true };

  /** One exchange at a time: a connection is a single conversation. */
  #lock: Promise<unknown> = Promise.resolve();
  /**
   * Set while a result set is open and unread. A second exchange cannot queue
   * behind it, because it finishes only when the caller drains it, and a caller
   * waiting on the queue never will — so it is refused, by name.
   */
  #streaming = false;

  constructor() {
    super({ dialect: MYSQL_DIALECT, backend: "mysql" });
  }

  override get usable(): boolean {
    return this.#fatal === null && this.#socket !== null;
  }

  /**
   * Fit for the next caller: usable, and not inside a transaction someone else
   * opened — which would otherwise leak into whoever borrowed it next.
   */
  override get reusable(): boolean {
    return this.usable && (this.#status & STATUS_IN_TRANS) === 0;
  }

  /** Whether the server is MariaDB, which speaks the same protocol with its own dialect. */
  get mariadb(): boolean {
    return this.serverVersion.includes("MariaDB");
  }

  // -- lifecycle ------------------------------------------------------------

  async open(options: MySqlOptions): Promise<void> {
    const budget = options.connectTimeout ?? 10_000;
    if (budget <= 0) return this.#open(options);
    let timer: ReturnType<typeof setTimeout> | undefined;
    const expired = new Promise<never>((_, reject) => {
      timer = setTimeout(() => {
        reject(
          new DbError(
            `the connection to ${options.host ?? "localhost"}:${options.port ?? 3306} did not complete within ${budget}ms`,
            { code: DbErrorCode.Timeout },
          ),
        );
      }, budget);
    });
    try {
      await Promise.race([this.#open(options), expired]);
    } catch (e) {
      // A socket half-open behind a rejected race is a descriptor a retry loop
      // would otherwise leak.
      await this.#teardown().catch(() => {});
      throw e;
    } finally {
      clearTimeout(timer);
    }
  }

  async #open(options: MySqlOptions): Promise<void> {
    this.#target = options;
    this.#decode = { temporal: options.temporal !== false };
    if (options.preparedStatementCacheSize !== undefined) {
      this.#cacheLimit = Math.max(0, Math.trunc(options.preparedStatementCacheSize));
    }
    const host = options.host ?? "localhost";
    const port = options.port ?? 3306;
    const sslmode = options.sslmode ?? "prefer";
    const wantsTls = sslmode !== "disable";
    const tlsOptions = options.sslRootCert === undefined ? {} : { ca: options.sslRootCert };

    let socket = netConnect(
      { hostname: host, port },
      wantsTls ? { secureTransport: "starttls", ...tlsOptions } : {},
    );
    this.#socket = socket;
    await socket.opened;
    let reader = new PacketReader(socket.readable);
    this.#reader = reader;

    // Through `#packet`, so a server that hangs up before greeting — one still
    // starting, or refusing this host — is a lost connection, not a bare Error.
    const greeting = await this.#packet();
    if (greeting[0] === 0xff) throw serverError(readError(greeting));
    const hello = readGreeting(greeting);
    this.serverVersion = hello.version;
    this.connectionId = hello.connectionId;

    let capabilities =
      CLIENT.LONG_PASSWORD |
      CLIENT.FOUND_ROWS |
      CLIENT.LONG_FLAG |
      CLIENT.PROTOCOL_41 |
      CLIENT.TRANSACTIONS |
      CLIENT.SECURE_CONNECTION |
      CLIENT.MULTI_STATEMENTS |
      CLIENT.MULTI_RESULTS |
      CLIENT.PS_MULTI_RESULTS |
      CLIENT.PLUGIN_AUTH |
      CLIENT.PLUGIN_AUTH_LENENC_CLIENT_DATA |
      CLIENT.DEPRECATE_EOF;
    if (options.database !== undefined && options.database !== "") {
      capabilities |= CLIENT.CONNECT_WITH_DB;
    }
    capabilities &= hello.capabilities | CLIENT.CONNECT_WITH_DB;
    if ((hello.capabilities & CLIENT.PROTOCOL_41) === 0) {
      throw new DbError("the server does not speak protocol 4.1, which MySQL has since 4.1", {
        code: DbErrorCode.Unsupported,
      });
    }
    // utf8mb4 in whichever collation the server prefers: MySQL 8's
    // `utf8mb4_0900_ai_ci` when it offered it, `utf8mb4_general_ci` otherwise,
    // which MariaDB and older MySQL both have.
    const charset = hello.charset === 255 ? 255 : 45;

    let seq = reader.seq;
    let tls = false;
    if (wantsTls && (hello.capabilities & CLIENT.SSL) !== 0) {
      capabilities |= CLIENT.SSL;
      // The same request as the start of a handshake response, cut short: the
      // server reads this much, then expects a TLS handshake.
      const request = new Writer(40)
        .u32(capabilities)
        .u32(0x0100_0000)
        .u8(charset)
        .zeros(23)
        .finish(++seq);
      const writer = socket.writable.getWriter();
      await writer.write(request);
      writer.releaseLock();
      socket = socket.startTls();
      this.#socket = socket;
      try {
        await socket.opened;
      } catch (e) {
        throw new DbError(
          `TLS with the server failed: ${e instanceof Error ? e.message : String(e)}. A stock MySQL server presents a certificate it signed itself, which cannot be verified — pass the server's authority as sslRootCert, or sslmode: "disable" to connect without TLS`,
          { code: DbErrorCode.ConnectionLost, cause: e },
        );
      }
      reader = new PacketReader(socket.readable);
      reader.seq = seq;
      this.#reader = reader;
      tls = true;
    } else if (sslmode === "require") {
      throw new DbError("the server does not offer TLS and sslmode is 'require'", {
        code: DbErrorCode.Unsupported,
      });
    }
    this.#writer = socket.writable.getWriter();
    this.#capabilities = capabilities;

    const password = options.password ?? "";
    const plugin = hello.plugin || "mysql_native_password";
    const response = new Writer(128)
      .u32(capabilities)
      .u32(0x0100_0000)
      .u8(charset)
      .zeros(23)
      .cstring(options.user ?? "root")
      .lenencBytes(await scrambleFor(plugin, password, hello.scramble));
    if ((capabilities & CLIENT.CONNECT_WITH_DB) !== 0) response.cstring(options.database ?? "");
    response.cstring(plugin);
    await this.#write(response.finish(reader.seq + 1));
    await this.#authenticate(plugin, password, hello.scramble, tls, options);

    // The session in UTC, so a TIMESTAMP arrives as the instant it is rather
    // than as a wall time in whatever zone the server was configured with.
    let init = "SET time_zone = '+00:00'";
    if (options.statementTimeout !== undefined && options.statementTimeout > 0) {
      const ms = Math.trunc(options.statementTimeout);
      init += this.mariadb ? `, max_statement_time = ${ms / 1000}` : `, max_execution_time = ${ms}`;
    }
    await this.#simple(init);
  }

  /** Answers the server until it says the login succeeded, or refuses it. */
  async #authenticate(
    plugin: string,
    password: string,
    scramble: Uint8Array,
    tls: boolean,
    options: MySqlOptions,
  ): Promise<void> {
    for (;;) {
      const packet = await this.#packet();
      switch (packet[0]) {
        case 0x00:
          this.#status = readOk(packet).status;
          return;
        case 0xff:
          throw serverError(readError(packet));
        case 0xfe: {
          // AuthSwitchRequest: the account uses another plugin than the one
          // the greeting named, and this is a fresh scramble for it.
          const p = new Payload(packet, 1);
          plugin = p.cstring();
          const data = p.restBytes();
          scramble = data[data.length - 1] === 0 ? data.subarray(0, data.length - 1) : data;
          await this.#reply(await scrambleFor(plugin, password, scramble));
          break;
        }
        case 0x01: {
          // AuthMoreData, which only `caching_sha2_password` sends.
          const data = packet.subarray(1);
          if (data.length === 1 && data[0] === 3) break; // fast auth succeeded; OK follows
          if (data.length === 1 && data[0] === 4) {
            // Full authentication: the server wants the password itself.
            if (tls) {
              await this.#reply(passwordBytes(password));
            } else if (options.serverPublicKey !== undefined) {
              await this.#reply(await encryptPassword(password, scramble, options.serverPublicKey));
            } else if (options.allowPublicKeyRetrieval === true) {
              // Plaintext, and the caller accepted the trade: ask for the key.
              await this.#reply(new Uint8Array([2]));
            } else {
              throw new DbError(
                "the server needs the password itself (caching_sha2_password full authentication), and this connection has no TLS to send it over. Connect with TLS, give the server's key as serverPublicKey, or — on a network you trust — allowPublicKeyRetrieval: true",
                { code: DbErrorCode.AuthFailed },
              );
            }
            break;
          }
          const pem = new TextDecoder().decode(data);
          if (pem.includes("BEGIN PUBLIC KEY")) {
            await this.#reply(await encryptPassword(password, scramble, pem));
            break;
          }
          throw new DbError(`the ${plugin} plugin sent something this driver does not understand`, {
            code: DbErrorCode.AuthFailed,
          });
        }
        default:
          throw new DbError(
            `unexpected packet 0x${packet[0]?.toString(16)} during authentication`,
            {
              code: DbErrorCode.AuthFailed,
            },
          );
      }
    }
  }

  /** One more packet in the conversation the server started. */
  async #reply(payload: Uint8Array): Promise<void> {
    await this.#write(new Writer(payload.length + 8).bytes(payload).finish(this.#reader!.seq + 1));
  }

  protected async _close(): Promise<void> {
    if (this.#fatal === null && this.#writer !== null) {
      try {
        await this.#write(new Writer(8).u8(COM.QUIT).finish(0));
      } catch {
        /* the peer may already be gone; the teardown below is what matters */
      }
    }
    await this.#teardown();
  }

  async #teardown(): Promise<void> {
    const [socket, reader, writer] = [this.#socket, this.#reader, this.#writer];
    this.#socket = null;
    this.#reader = null;
    this.#writer = null;
    try {
      writer?.releaseLock();
      await reader?.cancel();
      await socket?.close();
    } catch {
      /* closing twice is not an error */
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
      this.#streaming = false;
      void this.#teardown();
    }
    return this.#fatal;
  }

  async #write(bytes: Uint8Array): Promise<void> {
    if (this.#fatal !== null) throw this.#fatal;
    const writer = this.#writer;
    if (writer === null)
      throw new DbError("the connection is closed", { code: DbErrorCode.Closed });
    try {
      await writer.write(bytes);
    } catch (e) {
      throw this.#die(e);
    }
  }

  #readerOrThrow(): PacketReader {
    if (this.#fatal !== null) throw this.#fatal;
    const reader = this.#reader;
    if (reader === null)
      throw new DbError("the connection is closed", { code: DbErrorCode.Closed });
    return reader;
  }

  async #packet(): Promise<Uint8Array> {
    const reader = this.#readerOrThrow();
    try {
      return await reader.packet();
    } catch (e) {
      throw this.#die(e);
    }
  }

  /** Takes the connection for one exchange, returning the release. */
  async #acquire(): Promise<() => void> {
    if (this.#fatal !== null) throw this.#fatal;
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

  // -- prepared statements ----------------------------------------------------

  #statements = new Map<string, Prepared>();
  #cacheLimit = 100;
  /** Statements evicted from the cache, closed with the next command sent. */
  #closing: number[] = [];

  /**
   * Prepares `text` once per connection. The answer names how many parameters
   * it takes; its columns are skipped, because every execution sends them
   * again — and the ones sent then are the ones that are true then.
   */
  async #prepare(text: string): Promise<Prepared> {
    const cached = this.#statements.get(text);
    if (cached !== undefined) {
      // Re-inserting moves it to the back, making eviction least-recently-used.
      this.#statements.delete(text);
      this.#statements.set(text, cached);
      return cached;
    }
    await this.#write(
      this.#command(new Writer(text.length + 16).u8(COM.STMT_PREPARE).string(text)),
    );
    const first = await this.#packet();
    if (first[0] === 0xff) throw serverError(readError(first));
    const p = new Payload(first, 1);
    const id = p.u32();
    const columns = p.u16();
    const params = p.u16();
    const eof = (this.#capabilities & CLIENT.DEPRECATE_EOF) === 0;
    const skip = params + (params > 0 && eof ? 1 : 0) + columns + (columns > 0 && eof ? 1 : 0);
    for (let i = 0; i < skip; i++) await this.#packet();
    const entry = { id, params, transient: this.#cacheLimit === 0 };
    if (!entry.transient) {
      while (this.#statements.size >= this.#cacheLimit) {
        const oldest = this.#statements.entries().next();
        if (oldest.done === true) break;
        this.#statements.delete(oldest.value[0]);
        this.#closing.push(oldest.value[1].id);
      }
      this.#statements.set(text, entry);
    }
    return entry;
  }

  /**
   * A command's bytes, preceded by a `COM_STMT_CLOSE` for each statement the
   * cache let go. Those get no answer, so they cost nothing to send ahead of
   * the next command and nothing is sent just for them.
   */
  #command(w: Writer): Uint8Array {
    const body = w.finish(0);
    if (this.#closing.length === 0) return body;
    const parts = this.#closing.map((id) => new Writer(16).u8(COM.STMT_CLOSE).u32(id).finish(0));
    this.#closing = [];
    parts.push(body);
    let length = 0;
    for (const part of parts) length += part.length;
    const out = new Uint8Array(length);
    let at = 0;
    for (const part of parts) {
      out.set(part, at);
      at += part.length;
    }
    return out;
  }

  /** Sends `COM_STMT_EXECUTE` and reads the head of what comes back. */
  async #execute(prepared: Prepared, params: unknown[]): Promise<Head> {
    if (params.length !== prepared.params) {
      throw new DbError(
        `the statement takes ${prepared.params} parameter${prepared.params === 1 ? "" : "s"} and was given ${params.length}`,
        { code: DbErrorCode.Backend },
      );
    }
    const w = new Writer(64).u8(COM.STMT_EXECUTE).u32(prepared.id).u8(0).u32(1);
    writeParams(w, params);
    await this.#write(this.#command(w));
    return this.#head();
  }

  /** Runs `text` through the text protocol, reading the head of the answer. */
  async #query(text: string): Promise<Head> {
    await this.#write(this.#command(new Writer(text.length + 8).u8(COM.QUERY).string(text)));
    return this.#head();
  }

  /** The start of a response: an `OK`, an error, or a result set's columns. */
  async #head(): Promise<Head> {
    const first = await this.#packet();
    if (first[0] === 0x00) return { ok: readOk(first) };
    if (first[0] === 0xff) throw serverError(readError(first));
    if (first[0] === 0xfb) {
      throw new DbError("the server asked to read a local file, which this driver never does", {
        code: DbErrorCode.Unsupported,
      });
    }
    const count = new Payload(first).count();
    const columns: Column[] = [];
    for (let i = 0; i < count; i++) columns.push(readColumn(await this.#packet()));
    if ((this.#capabilities & CLIENT.DEPRECATE_EOF) === 0) await this.#packet();
    return { columns };
  }

  /**
   * The head for a statement, prepared where MySQL can prepare it and run as
   * text where it cannot. A few statements — `USE`, some `SHOW`s, `XA` — are
   * refused by the prepared protocol; with no parameters there is nothing the
   * text protocol would get wrong, so they run there instead.
   */
  async #run(text: string, params: unknown[]): Promise<{ head: Head; binary: boolean }> {
    let prepared: Prepared;
    try {
      prepared = await this.#prepare(text);
    } catch (e) {
      if (
        params.length === 0 &&
        (e as { server?: ServerError }).server?.code === ER_UNSUPPORTED_PS
      ) {
        return { head: await this.#query(text), binary: false };
      }
      throw e;
    }
    try {
      return { head: await this.#execute(prepared, params), binary: true };
    } finally {
      if (prepared.transient) this.#closing.push(prepared.id);
    }
  }

  // -- results ----------------------------------------------------------------

  /** Reads rows until the batch is full or the result set ends. */
  async #batch(layout: Int8Array, binary: boolean): Promise<Batch> {
    const reader = this.#reader;
    const batch = new RowBatch(Math.min(reader?.buffered ?? 0, BATCH_BYTES));
    const append = binary
      ? (bytes: Uint8Array, view: DataView, start: number, length: number) => {
          batch.appendBinaryRow(bytes, view, start, length, layout);
          return batch.size < BATCH_BYTES;
        }
      : null;
    const done = (finished: boolean): Batch => ({
      bytes: batch.gathered,
      rows: batch.count,
      done: finished,
    });
    for (;;) {
      // Every row that has already arrived, in one synchronous pass — only
      // what has not arrived yet, or is not a row, costs a promise.
      if (append !== null && reader !== null && this.#fatal === null) {
        reader.take(0x00, append);
        if (batch.size >= BATCH_BYTES) return done(false);
      }
      const packet = await this.#packet();
      const lead = packet[0];
      if (lead === 0xfe && packet.length < 0xff_ffff) {
        const status =
          (this.#capabilities & CLIENT.DEPRECATE_EOF) !== 0
            ? readOk(packet).status
            : new Payload(packet, 3).u16();
        this.#status = status;
        if ((status & STATUS_MORE_RESULTS) !== 0) await this.#discardResults();
        return done(true);
      }
      if (lead === 0xff) {
        throw serverError(readError(packet));
      }
      const view = new DataView(packet.buffer, packet.byteOffset, packet.byteLength);
      if (binary) {
        batch.appendBinaryRow(packet, view, 0, packet.length, layout);
      } else {
        const row = textRowAsBinary(packet, layout.length);
        const rowView = new DataView(row.buffer, row.byteOffset, row.byteLength);
        batch.appendBinaryRow(row, rowView, 0, row.length, layout);
      }
      if (batch.size >= BATCH_BYTES) return done(false);
    }
  }

  /**
   * Reads and discards any further result sets — a `CALL` answers with its
   * procedure's results and then its own status. The first is the caller's;
   * the rest have nowhere to go, and must come off the wire before anything
   * else can be asked.
   */
  async #discardResults(): Promise<void> {
    for (;;) {
      const head = await this.#head();
      if ("ok" in head) {
        this.#status = head.ok.status;
        if ((head.ok.status & STATUS_MORE_RESULTS) === 0) return;
        continue;
      }
      for (;;) {
        const packet = await this.#packet();
        if (packet[0] === 0xff) throw serverError(readError(packet));
        if (packet[0] === 0xfe && packet.length < 0xff_ffff) {
          const status =
            (this.#capabilities & CLIENT.DEPRECATE_EOF) !== 0
              ? readOk(packet).status
              : new Payload(packet, 3).u16();
          this.#status = status;
          if ((status & STATUS_MORE_RESULTS) === 0) return;
          break;
        }
      }
    }
  }

  // -- the runtime:db contract ---------------------------------------------------

  #sql(q: NormalizedQuery): { text: string; positional: unknown[] } {
    if (q.named.length > 0) {
      throw new DbError(
        "MySQL binds parameters by position; pass an array and use ? placeholders (or the sql`` tag)",
        { code: DbErrorCode.Unsupported },
      );
    }
    return { text: q.text ?? "", positional: q.positional };
  }

  protected async _query(query: NormalizedQuery): Promise<Rows<MySqlRow>> {
    const q = this.#sql(query);
    const release = await this.#acquire();
    let held = true;
    const releaseOnce = () => {
      if (!held) return;
      held = false;
      this.#streaming = false;
      release();
    };
    try {
      const { head, binary } = await this.#run(q.text, q.positional);
      if ("ok" in head) {
        // A statement with no result set, run through `query()`.
        this.#status = head.ok.status;
        if ((head.ok.status & STATUS_MORE_RESULTS) !== 0) await this.#discardResults();
        releaseOnce();
        return new Rows(emptySource(), defineRowShape([]));
      }
      const layout = binary ? widths(head.columns) : textLayout(head.columns.length);
      const shape = rowShape(head.columns, this.#decode, binary);

      let first: Batch | null = await this.#batch(layout, binary);
      if (first.done) {
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
              const batch = await self.#batch(layout, binary);
              if (batch.done) releaseOnce();
              return batch;
            } catch (e) {
              releaseOnce();
              throw e;
            }
          },
          async close(): Promise<void> {
            if (!held) return;
            // A caller that stopped early left the server mid-result, and the
            // rest has to come off the wire before anything else can be asked.
            try {
              for (;;) {
                const batch = await self.#batch(layout, binary);
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

  protected async _execute(
    query: NormalizedQuery,
  ): Promise<{ changes: number; lastInsertRowid: number | null }> {
    const q = this.#sql(query);
    const release = await this.#acquire();
    try {
      const { head, binary } = await this.#run(q.text, q.positional);
      if ("ok" in head) {
        this.#status = head.ok.status;
        if ((head.ok.status & STATUS_MORE_RESULTS) !== 0) await this.#discardResults();
        return {
          changes: head.ok.affectedRows,
          lastInsertRowid: head.ok.lastInsertId === 0 ? null : head.ok.lastInsertId,
        };
      }
      // A statement that returned rows, run for its effect: the rows are read
      // and dropped, which is what running it to completion means.
      const layout = binary ? widths(head.columns) : textLayout(head.columns.length);
      while (!(await this.#batch(layout, binary)).done) {
        /* drained */
      }
      return { changes: 0, lastInsertRowid: null };
    } finally {
      release();
    }
  }

  /** Runs one statement through the text protocol, discarding any rows. */
  async #simple(text: string): Promise<Ok> {
    const head = await this.#query(text);
    if ("ok" in head) {
      this.#status = head.ok.status;
      if ((head.ok.status & STATUS_MORE_RESULTS) !== 0) await this.#discardResults();
      return head.ok;
    }
    while (!(await this.#batch(textLayout(head.columns.length), false)).done) {
      /* drained */
    }
    return { affectedRows: 0, lastInsertId: 0, status: this.#status };
  }

  /**
   * Runs a script — several statements in one string — through the text
   * protocol, which takes more than one statement where a prepared statement
   * takes exactly one.
   *
   * **No parameters**: the text protocol has nowhere to put them, so anything
   * variable would have to be quoted into the SQL, and that is how injection
   * happens. Use it for schema and fixed statements. Unlike PostgreSQL, MySQL
   * does **not** wrap a script in a transaction, and DDL commits implicitly —
   * a failure part-way leaves everything before it done.
   *
   * Rows are discarded: this reports what each statement did.
   */
  async executeScript(
    sql: string,
    options: { signal?: AbortSignal } = {},
  ): Promise<{ changes: number; lastInsertRowid: number | null }[]> {
    this._open();
    return this._withSignal(options.signal, async () => {
      const release = await this.#acquire();
      try {
        const results: { changes: number; lastInsertRowid: number | null }[] = [];
        let head = await this.#query(sql);
        for (;;) {
          let status: number;
          if ("ok" in head) {
            status = head.ok.status;
            results.push({
              changes: head.ok.affectedRows,
              lastInsertRowid: head.ok.lastInsertId === 0 ? null : head.ok.lastInsertId,
            });
          } else {
            for (;;) {
              const packet = await this.#packet();
              if (packet[0] === 0xff) throw serverError(readError(packet));
              if (packet[0] === 0xfe && packet.length < 0xff_ffff) {
                status =
                  (this.#capabilities & CLIENT.DEPRECATE_EOF) !== 0
                    ? readOk(packet).status
                    : new Payload(packet, 3).u16();
                break;
              }
            }
            results.push({ changes: 0, lastInsertRowid: null });
          }
          this.#status = status;
          if ((status & STATUS_MORE_RESULTS) === 0) return results;
          head = await this.#head();
        }
      } finally {
        release();
      }
    });
  }

  /**
   * MySQL's spelling of the three statements a transaction is made of — which
   * differs from the SQL standard's at `RELEASE SAVEPOINT`. Sent as text: the
   * prepared protocol would cost a round trip to prepare each, for statements
   * that have no parameters to bind.
   */
  protected override async _beginTransaction({
    nested,
    name,
  }: {
    nested: boolean;
    name: string | null;
  }): Promise<void> {
    await this.#exchange(nested ? `SAVEPOINT ${name}` : "BEGIN");
  }

  protected override async _commitTransaction({
    nested,
    name,
  }: {
    nested: boolean;
    name: string | null;
  }): Promise<void> {
    await this.#exchange(nested ? `RELEASE SAVEPOINT ${name}` : "COMMIT");
  }

  protected override async _rollbackTransaction({
    nested,
    name,
  }: {
    nested: boolean;
    name: string | null;
  }): Promise<void> {
    await this.#exchange(nested ? `ROLLBACK TO SAVEPOINT ${name}` : "ROLLBACK");
  }

  async #exchange(text: string): Promise<void> {
    const release = await this.#acquire();
    try {
      await this.#simple(text);
    } finally {
      release();
    }
  }

  /** The base class's cancellation hook: this connection's own `cancel()`. */
  protected override async _cancel(): Promise<void> {
    await this.cancel();
  }

  /**
   * Stops the statement this connection is running, if there is one.
   *
   * MySQL has no cancel message on the connection itself — it is reading the
   * statement's answer — so this opens a second connection to the same server
   * as the same user and runs `KILL QUERY` naming this one. The statement fails
   * with `ER_QUERY_INTERRUPTED` and this connection stays open and usable.
   */
  async cancel(): Promise<void> {
    if (this.#fatal !== null || this.#socket === null) return;
    const killer = new MySqlConnection();
    try {
      await killer.open({ ...this.#target, preparedStatementCacheSize: 0 });
      await killer.#simple(`KILL QUERY ${this.connectionId}`);
    } finally {
      await killer.close().catch(() => {});
    }
  }

  /** Asks the server whether it is still there. */
  async ping(): Promise<void> {
    this._open();
    const release = await this.#acquire();
    try {
      await this.#write(new Writer(8).u8(COM.PING).finish(0));
      const packet = await this.#packet();
      if (packet[0] === 0xff) throw serverError(readError(packet));
    } finally {
      release();
    }
  }
}

// ---------------------------------------------------------------------------
// Handshake
// ---------------------------------------------------------------------------

interface Greeting {
  version: string;
  connectionId: number;
  scramble: Uint8Array;
  capabilities: number;
  charset: number;
  plugin: string;
}

/** Reads the server's Initial Handshake (protocol 10). */
function readGreeting(packet: Uint8Array): Greeting {
  const p = new Payload(packet);
  const protocol = p.u8();
  if (protocol !== 10) {
    throw new DbError(`the server speaks handshake protocol ${protocol}; this driver speaks 10`, {
      code: DbErrorCode.Unsupported,
    });
  }
  const version = p.cstring();
  const connectionId = p.u32();
  const part1 = p.bytesOf(8);
  p.u8(); // filler
  let capabilities = p.u16();
  const charset = p.u8();
  p.u16(); // status
  capabilities |= p.u16() << 16;
  const dataLength = p.u8();
  p.bytesOf(10); // reserved
  let part2: Uint8Array = new Uint8Array(0);
  if ((capabilities & CLIENT.SECURE_CONNECTION) !== 0) {
    part2 = p.bytesOf(Math.max(13, dataLength - 8));
    // The second part is NUL-terminated; the scramble is the twenty bytes before it.
    if (part2[part2.length - 1] === 0) part2 = part2.subarray(0, part2.length - 1);
  }
  const plugin = (capabilities & CLIENT.PLUGIN_AUTH) !== 0 && !p.done ? p.cstring() : "";
  const scramble = new Uint8Array(part1.length + part2.length);
  scramble.set(part1);
  scramble.set(part2, part1.length);
  return { version, connectionId, scramble, capabilities: capabilities >>> 0, charset, plugin };
}

/** The authentication response `plugin` computes for this password. */
async function scrambleFor(
  plugin: string,
  password: string,
  scramble: Uint8Array,
): Promise<Uint8Array> {
  switch (plugin) {
    case "caching_sha2_password":
      return cachingSha2(password, scramble);
    case "mysql_native_password":
      return nativePassword(password, scramble);
    case "mysql_clear_password":
      throw new DbError(
        "the server asked for the password in clear text (mysql_clear_password), which this driver never sends",
        { code: DbErrorCode.AuthFailed },
      );
    default:
      throw new DbError(
        `the server asked for the ${plugin} authentication plugin, which this driver does not speak`,
        {
          code: DbErrorCode.AuthFailed,
        },
      );
  }
}

// ---------------------------------------------------------------------------
// Packets
// ---------------------------------------------------------------------------

function readOk(packet: Uint8Array): Ok {
  const p = new Payload(packet, 1);
  const affectedRows = p.count();
  const lastInsertId = p.count();
  const status = p.done ? 0 : p.u16();
  return { affectedRows, lastInsertId, status };
}

function readError(packet: Uint8Array): ServerError {
  const p = new Payload(packet, 1);
  const code = p.u16();
  let sqlstate = "HY000";
  if (packet[p.at] === 0x23) {
    // '#', then five characters of SQLSTATE.
    p.u8();
    sqlstate = new TextDecoder().decode(p.bytesOf(5));
  }
  return { code, sqlstate, message: p.rest() };
}

function serverError(server: ServerError): DbError {
  const error = asDbError(
    Object.assign(new Error(server.message), { code: String(server.code) }),
    portableCode(server),
  );
  // Everything the server said, kept: the SQLSTATE and the numeric code are
  // what an application matches on when the portable code is too coarse.
  return Object.assign(error, { server });
}

// ---------------------------------------------------------------------------
// Row shapes
// ---------------------------------------------------------------------------

/**
 * The row class for a result of this shape — one per shape for the whole
 * process, not one per query or per connection, so a line of the caller's code
 * reading `row.id` sees one class and stays fast.
 */
function rowShape(columns: Column[], options: DecodeOptions, binary: boolean) {
  const key = `${binary ? "B" : "S"}${shapeKey(columns, options)}`;
  let shape = SHAPES.get(key);
  if (shape === undefined) {
    shape = defineRowShape(
      columns.map((column) => ({ name: column.name, declType: null, type: column.type })),
      {
        decoders: columns.map((column) =>
          binary ? decoderFor(column, options) : textDecoderFor(column),
        ),
      },
    );
    // Bounded, oldest first: a program generating SQL can produce shapes
    // without end, and a cache that only grows is a leak with a hit rate.
    if (SHAPES.size >= SHAPE_LIMIT) SHAPES.delete(SHAPES.keys().next().value!);
  } else {
    SHAPES.delete(key);
  }
  SHAPES.set(key, shape);
  return shape;
}

const SHAPES = new Map<string, ReturnType<typeof defineRowShape>>();
const SHAPE_LIMIT = 1000;

// ---------------------------------------------------------------------------
// The text protocol, for the statements MySQL will not prepare
// ---------------------------------------------------------------------------

/** A text row's layout: every column is a length-encoded string. */
function textLayout(columns: number): Int8Array {
  return new Int8Array(columns);
}

/**
 * A text-protocol row rewritten as a binary one, so one transcoder serves
 * both. A text row is a length-encoded string per column with `0xFB` for NULL;
 * a binary row is a NULL bitmap and then the same strings.
 */
function textRowAsBinary(packet: Uint8Array, columns: number): Uint8Array {
  const bitmapLength = (columns + 9) >> 3;
  const out = new Uint8Array(1 + bitmapLength + packet.length);
  let w = 1 + bitmapLength;
  const p = new Payload(packet);
  for (let c = 0; c < columns; c++) {
    const start = p.at;
    if (packet[start] === 0xfb) {
      p.u8();
      const bit = c + 2;
      out[1 + (bit >> 3)]! |= 1 << (bit & 7);
      continue;
    }
    p.skipLenenc();
    out.set(packet.subarray(start, p.at), w);
    w += p.at - start;
  }
  return out.subarray(0, w);
}

const DECODER = new TextDecoder();

/** Text-protocol values are text; numbers are parsed and bytes kept as bytes. */
function textDecoderFor(column: Column) {
  const text = (bytes: Uint8Array, _view: DataView, start: number, length: number) =>
    DECODER.decode(bytes.subarray(start, start + length));
  switch (column.type) {
    case T.TINY:
    case T.SHORT:
    case T.LONG:
    case T.INT24:
    case T.YEAR:
    case T.FLOAT:
    case T.DOUBLE:
      return (bytes: Uint8Array, view: DataView, start: number, length: number) =>
        Number(text(bytes, view, start, length));
    case T.LONGLONG:
      return (bytes: Uint8Array, view: DataView, start: number, length: number) => {
        const value = BigInt(text(bytes, view, start, length));
        return value >= -9007199254740991n && value <= 9007199254740991n ? Number(value) : value;
      };
    case T.JSON:
      return (bytes: Uint8Array, view: DataView, start: number, length: number) =>
        JSON.parse(text(bytes, view, start, length));
    default:
      return column.charset === 63
        ? (bytes: Uint8Array, _view: DataView, start: number, length: number) =>
            bytes.slice(start, start + length)
        : text;
  }
}

// ---------------------------------------------------------------------------
// Result sources
// ---------------------------------------------------------------------------

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

function emptySource() {
  return oneBatch(NOTHING);
}
