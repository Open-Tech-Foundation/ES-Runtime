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
import * as msg from "./protocol/messages.js";
import { AUTH, B } from "./protocol/messages.js";
import { scram } from "./protocol/scram.js";
import { portableCode, type ServerMessage } from "./protocol/errors.js";
import { decoderFor, encodeParam } from "./protocol/values.js";

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
  /** `"require"` (default) upgrades to TLS; `"disable"` stays in plaintext. */
  sslmode?: "require" | "prefer" | "disable";
}

interface Batch {
  bytes: Uint8Array;
  rows: number;
  done: boolean;
}

export class PgConnection extends BaseConnection {
  #socket: Awaited<ReturnType<typeof netConnect>> | null = null;
  #frames: FrameReader | null = null;
  #writer: WritableStreamDefaultWriter<Uint8Array> | null = null;
  /** Server parameters from the handshake (`server_version`, and so on). */
  readonly parameters: Record<string, string> = {};
  /** The last `ReadyForQuery` status: `I` idle, `T` in a transaction, `E` failed. */
  status = "I";
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
    const host = options.host ?? "localhost";
    const port = options.port ?? 5432;
    const sslmode = options.sslmode ?? "prefer";
    const wantsTls = sslmode !== "disable";

    let socket = netConnect(
      { hostname: host, port },
      wantsTls ? { secureTransport: "starttls" } : {},
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

    this.#socket = socket;
    this.#frames = frames ?? new FrameReader(socket.readable);
    this.#writer = socket.writable.getWriter();

    await this.#send(
      msg.startup({
        user: options.user ?? "postgres",
        database: options.database ?? options.user ?? "postgres",
        application_name: options.applicationName ?? "esrun",
        client_encoding: "UTF8",
      }),
    );
    await this.#authenticate(options);
  }

  async #send(bytes: Uint8Array): Promise<void> {
    const writer = this.#writer;
    if (writer === null) throw new DbError("the connection is closed", { code: DbErrorCode.Closed });
    await writer.write(bytes);
  }

  async #next(): Promise<{ tag: number; frame: Uint8Array }> {
    const frames = this.#frames;
    if (frames === null) throw new DbError("the connection is closed", { code: DbErrorCode.Closed });
    return frames.message();
  }

  /**
   * Takes the connection for one exchange, returning the release.
   *
   * Every caller must release in a `finally`: a holder that throws without
   * releasing would leave the chain waiting on a promise nobody settles, and
   * the connection would be lost rather than merely broken.
   */
  async #acquire(): Promise<() => void> {
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
        case B.ParameterStatus: {
          this.parameters[fields.cstring()] = fields.cstring();
          break;
        }
        case B.BackendKeyData:
          this.#processId = fields.i32();
          this.#secretKey = fields.i32();
          break;
        case B.NoticeResponse:
          break;
        case B.ErrorResponse:
          throw serverError(readServerMessage(fields));
        case B.ReadyForQuery:
          this.status = String.fromCharCode(fields.u8());
          return;
        default:
          break;
      }
    }
  }

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

  /** Sends one extended-protocol round: parse, bind, describe, execute, sync. */
  async #ask(text: string, params: unknown[]): Promise<void> {
    const bound = params.map((value) => encodeParam(value));
    await this.#send(
      msg.concat([
        msg.parse("", text),
        msg.bind("", "", bound),
        msg.describePortal(""),
        msg.execute("", 0),
        msg.sync(),
      ]),
    );
  }

  /**
   * Reads until the statement's rows begin, returning the column description —
   * or `null` when the statement returns no rows at all.
   */
  async #describe(): Promise<{ names: string[]; oids: number[] } | null> {
    for (;;) {
      const { tag, frame } = await this.#next();
      const fields = new Fields(frame);
      switch (tag) {
        case B.ParseComplete:
        case B.BindComplete:
          break;
        case B.RowDescription: {
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
        case B.NoData:
          return null;
        case B.ErrorResponse: {
          const error = readServerMessage(fields);
          await this.#drainToReady();
          throw serverError(error);
        }
        case B.NoticeResponse:
        case B.ParameterStatus:
          break;
        default:
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
      await this.#ask(q.text, q.positional);
      const described = await this.#describe();
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
        // makes possible.
        oid: described.oids[i]!,
      }));
      const shape = defineRowShape(columns, {
        decoders: described.oids.map((oid) => decoderFor(oid)),
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
      await this.#ask(q.text, q.positional);
      await this.#describe();
      await this.#batch(0);
      return { changes: affectedRows(this.#lastTag), lastInsertRowid: null };
    } finally {
      release();
    }
  }

  #rejectNamed(named: [string, unknown][]): void {
    if (named.length > 0) {
      throw new DbError(
        "PostgreSQL binds parameters by position; pass an array and use $1, $2, … (or the sql`` tag)",
        { code: DbErrorCode.Unsupported },
      );
    }
  }

  protected async _close(): Promise<void> {
    try {
      await this.#send(msg.terminate());
    } catch {
      /* the peer may already be gone; the close below is what matters */
    }
    try {
      this.#writer?.releaseLock();
      await this.#frames?.cancel();
      await this.#socket?.close();
    } catch {
      /* closing twice is not an error */
    }
    this.#socket = null;
    this.#frames = null;
    this.#writer = null;
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
