declare module "runtime:db" {
  // ---------------------------------------------------------------------
  // Application tier
  // ---------------------------------------------------------------------

  /** A value a statement can bind. `bigint` binds as a 64-bit integer. */
  export type DbInput =
    | null
    | undefined
    | boolean
    | number
    | bigint
    | string
    | Date
    | Uint8Array
    | ArrayBuffer
    | ArrayBufferView;

  /** A value a column can produce. Integers outside `Number.MAX_SAFE_INTEGER` arrive as `bigint`. */
  export type DbOutput = null | number | bigint | string | Uint8Array;

  /** Parameters: an array binds by position, an object binds by name. */
  export type DbParams = readonly DbInput[] | Record<string, DbInput>;

  /** The portable error classification. A backend's own code stays on {@link DbError.backendCode}. */
  export const DbErrorCode: {
    readonly UniqueViolation: "ERR_DB_UNIQUE_VIOLATION";
    readonly ForeignKeyViolation: "ERR_DB_FOREIGN_KEY_VIOLATION";
    readonly NotNullViolation: "ERR_DB_NOT_NULL_VIOLATION";
    readonly CheckViolation: "ERR_DB_CHECK_VIOLATION";
    readonly Deadlock: "ERR_DB_DEADLOCK";
    readonly SerializationFailure: "ERR_DB_SERIALIZATION_FAILURE";
    readonly Busy: "ERR_DB_BUSY";
    readonly ConnectionLost: "ERR_DB_CONNECTION_LOST";
    readonly AuthFailed: "ERR_DB_AUTH_FAILED";
    readonly Timeout: "ERR_DB_TIMEOUT";
    readonly Syntax: "ERR_DB_SYNTAX";
    readonly UndefinedTable: "ERR_DB_UNDEFINED_TABLE";
    readonly UndefinedColumn: "ERR_DB_UNDEFINED_COLUMN";
    readonly ReadOnly: "ERR_DB_READ_ONLY";
    readonly QueryForm: "ERR_DB_QUERY_FORM";
    readonly Unsupported: "ERR_DB_UNSUPPORTED";
    readonly Closed: "ERR_DB_CLOSED";
    readonly ConnectionBusy: "ERR_DB_CONNECTION_BUSY";
    readonly Backend: "ERR_DB_BACKEND";
  };

  /**
   * A database failure.
   *
   * `code` is layered: the driver's own classification wins, then a stable host
   * code (`ERR_CAPABILITY_DENIED`, `ERR_JAIL_ESCAPE`, …) if there is one, then
   * `ERR_DB_BACKEND`. A denied capability stays a denied capability.
   */
  export class DbError extends Error {
    constructor(
      message: string,
      options?: { code?: string; backendCode?: string; cause?: unknown },
    );
    readonly code: string;
    /** The backend's own code, where it had one. */
    readonly backendCode?: string;
  }

  /** A column of a result set, described once per query. */
  export interface Column {
    readonly name: string;
    /** The declared type, where the backend reports one. Advisory. */
    readonly declType: string | null;
  }

  /**
   * A result row: a **lazy view** over its batch, with one getter per column,
   * so a column nobody reads costs nothing.
   *
   * Because the getters live on the prototype, `{ ...row }` does **not** copy
   * the columns — use {@link Row.toObject}. Nothing internal leaks through a
   * spread either; it yields an empty object.
   */
  export interface Row {
    readonly [column: string]: DbOutput | (() => unknown);
    /** The columns as an array, in query order. */
    values(): DbOutput[];
    /** The row as a plain object — how a row is materialized. */
    toObject(): Record<string, DbOutput>;
    toJSON(): Record<string, DbOutput>;
  }

  /**
   * An async-iterable result set, pulled one batch at a time. Never the whole
   * result: a table larger than memory streams through at the cost of a batch.
   * Stopping early closes the cursor.
   */
  /**
   * Where a `Rows` gets its batches. A driver supplies one of these; the
   * iteration, early-exit and close discipline come from `Rows` itself.
   */
  export interface RowSource {
    /**
     * The next batch. `done` ends the result.
     *
     * A batch arrives one of two ways, and which one is the backend's nature
     * rather than its choice. `bytes` + `rows` is the shared layout, for
     * anything that read its values off a socket or out of an engine — pair it
     * with {@link defineRowShape}. `records` is for a backend whose values are
     * already JavaScript, and each record is an array of values in column
     * order — pair it with {@link defineRecordShape}.
     */
    next(
      maxBytes: number,
    ): Promise<
      | { bytes: Uint8Array; rows: number; done: boolean; records?: undefined }
      | { records: readonly unknown[][]; done: boolean }
    >;
    /** Called once, however the iteration ended. */
    close(): Promise<void>;
    /** `true` when the backend finished the result without opening a cursor. */
    exhausted?: boolean;
  }

  export class Rows implements AsyncIterable<Row> {
    constructor(source: RowSource, shape: RowShape | RecordShape);
    /**
     * Rows already in hand, as a result set — for a backend whose answer is
     * JavaScript rather than bytes. The records are wrapped, not encoded and
     * decoded. `columns` defaults to the union of the records' keys, in
     * first-seen order.
     */
    static fromObjects(
      records: Iterable<Record<string, unknown>>,
      columns?: readonly Column[],
    ): Rows;
    [Symbol.asyncIterator](): AsyncIterator<Row>;
    readonly columns: readonly Column[];
    /**
     * `true` when the backend finished this result without leaving a cursor
     * open — the rows came back with the query itself, so there is nothing to
     * fetch and nothing to close.
     */
    readonly exhausted: boolean;
    /** The whole result set as an array. */
    toArray(): Promise<Row[]>;
    /** The first row, or `null`. Closes the cursor without reading the rest. */
    first(): Promise<Row | null>;
    close(): Promise<void>;
  }

  /** What a statement run for its effect did. */
  export interface ExecuteResult {
    readonly changes: number;
    /**
     * The key the backend generated, where it generated one, and `null`
     * everywhere else — which includes every backend that has no such concept
     * and every insert that did not make one. SQLite's word for it, because
     * SQLite is where the concept comes from; the type is wider than SQLite's
     * because a generated key elsewhere is as often a string (a document id, a
     * UUID) as an integer. PostgreSQL always reports `null` and expects
     * `RETURNING`.
     */
    readonly lastInsertRowid: number | bigint | string | null;
  }

  /** A query built by the {@link sql} tag: fragments and values, kept apart. */
  export class Query {
    render(dialect: Dialect): { text: string; params: DbInput[] };
  }

  /** A structured query for a backend that takes an AST rather than SQL text. */
  export interface QueryAst {
    readonly __queryAst: true;
    readonly ast: unknown;
  }

  /** Anything `query`/`execute` accepts. */
  export type Queryable = string | Query | QueryAst;

  /** Per-call options every backend takes. */
  export interface CallOptions {
    /**
     * Cancels the call. The backend is asked to abandon the statement and the
     * connection is left usable; the rejection carries the signal's `reason`.
     */
    signal?: AbortSignal;
  }

  /** An open connection. */
  export interface Connection {
    readonly dialect: Dialect;
    /** The backend's name, e.g. `"sqlite"`. */
    readonly backend: string;
    query(q: Queryable, params?: DbParams, options?: CallOptions): Promise<Rows>;
    execute(q: Queryable, params?: DbParams, options?: CallOptions): Promise<ExecuteResult>;
    /**
     * Runs one statement against many parameter sets, crossing the boundary
     * once and preparing once. Runs as a single transaction unless one is
     * already open, in which case it joins that one.
     */
    executeMany(q: string | Query, rows: readonly DbParams[]): Promise<ExecuteResult>;
    /**
     * Runs `fn` in a transaction, committing when it returns and rolling back
     * when it throws. Nested calls become savepoints where the backend has
     * them, so a helper that opens one composes with a caller that already did.
     */
    transaction<T>(fn: (tx: Connection) => Promise<T>): Promise<T>;
    /**
     * Runs `fn` with a connection held for the whole of it.
     *
     * On a single connection that connection is the receiver; on a pool it is
     * one borrowed for the duration. It is on both so that code which must not
     * be spread over two connections — a session setting, a `LISTEN`, a
     * `WATCH`, a temporary table — does not have to know which kind it holds.
     */
    withConnection<T>(fn: (connection: Connection) => Promise<T>): Promise<T>;
    /** Whether this is still worth using at all. */
    readonly usable: boolean;
    /** Whether this is fit for the next caller — what a pool asks before reusing one. */
    readonly reusable: boolean;
    close(): Promise<void>;
    [Symbol.asyncDispose](): Promise<void>;
  }

  /** How big a pool is and how long it waits. */
  export interface PoolSettings {
    /** Connections to open at most. Default 10. */
    max?: number;
    /** How long an unused connection is kept, in ms. Default 30 000; `0` never reaps. */
    idleTimeout?: number;
    /** How long to wait for a free connection, in ms. Default 10 000; `0` waits forever. */
    acquireTimeout?: number;
  }

  /** Options every {@link connect} takes, whatever the driver. */
  export interface ConnectOptions {
    /** The driver to open with. Required. */
    driver: AnyDriver;
    /** Pool instead of opening one connection. `true` takes the defaults. */
    pool?: boolean | PoolSettings;
  }

  /** Options for the built-in {@link sqlite} driver. */
  export interface SqliteOptions {
    /**
     * Encryption key — hex string or bytes. **Never put a key in the connection
     * string**; one passed as a URL parameter is refused, because a key in a URL
     * ends up in logs, error messages and stack traces.
     */
    key?: string | Uint8Array | ArrayBuffer | ArrayBufferView;
    /** Cipher name; defaults to the backend's. */
    cipher?: string;
    /** Open without the ability to write. */
    readOnly?: boolean;
  }

  /**
   * Opens a connection with the driver you pass.
   *
   * ```js
   * import { connect, sqlite } from "runtime:db";
   * const db = await connect("sqlite:./app.db", { driver: sqlite });
   *
   * import postgres from "@opentf/esrun-postgres";
   * const pg = await connect("postgres://user@host/app", { driver: postgres });
   * ```
   *
   * What comes back is **that driver's** connection, so a driver's own surface
   * — Redis's commands, PostgreSQL's `LISTEN` — is on the object `connect`
   * returned and needs no second entry point to reach.
   *
   * `pool: true` gives a pool presenting the same surface one connection does.
   *
   * `sqlite:` names a file format and a SQL dialect the way `postgres://` names
   * a wire protocol — not an implementation, which may change without the URL
   * changing. `sqlite::memory:` opens a database that exists only in memory and
   * needs no capability; every other `sqlite:` open is scoped by `--allow-read`
   * / `--allow-write` exactly as a file is.
   */
  export function connect<C extends Connection, O, P>(
    url: string,
    options: O & { driver: Driver<C, O, P>; pool: true | PoolSettings },
  ): Promise<Awaited<P>>;
  export function connect<C extends Connection, O, P>(
    url: string,
    options: O & { driver: Driver<C, O, P>; pool?: false },
  ): Promise<C>;

  /** The built-in SQLite driver — an ordinary driver, passed the ordinary way. */
  export const sqlite: Driver<Connection, SqliteOptions, PooledConnection>;

  /**
   * The `sql` tagged template: every interpolation becomes a parameter, never
   * text. A nested `Query` splices with its own values, so fragments compose.
   *
   * ```js
   * await db.query(sql`SELECT * FROM users WHERE id = ${id}`);
   * ```
   */
  export function sql(strings: TemplateStringsArray, ...values: DbInput[]): Query;

  /** Marks a structured query for a backend that takes an AST. */
  export function queryAst(ast: unknown): QueryAst;

  // ---------------------------------------------------------------------
  // Driver tier — for building a backend or an ORM on top
  // ---------------------------------------------------------------------

  /** How a backend spells a placeholder and quotes an identifier. */
  export class Dialect {
    constructor(options: {
      name: string;
      placeholder: (index: number) => string;
      quote?: string;
      supports?: Partial<DialectSupport>;
    });
    readonly name: string;
    readonly supports: Readonly<DialectSupport>;
    /** The placeholder for the 1-based parameter `index`. */
    placeholder(index: number): string;
    /** Quotes an identifier, doubling the quote character inside it. */
    quoteIdent(name: string): string;
  }

  export interface DialectSupport {
    /**
     * A driver may declare capabilities of its own beside these — a vector
     * index, full-text search, a graph traversal — and they survive on
     * `dialect.supports` unchanged. The names here are the ones the kit itself
     * acts on; anything else is a backend telling an ORM what it can do, which
     * is the only way an ORM can branch on a backend that did not exist when
     * the ORM was written.
     */
    [capability: string]: boolean | undefined;
    returning: boolean;
    savepoints: boolean;
    namedParameters: boolean;
    /**
     * The backend takes SQL text and `` sql`` `` templates. Default `true`.
     *
     * A backend that says `false` refuses text with `ERR_DB_QUERY_FORM`, the
     * same way a SQL backend refuses an AST.
     */
    sqlText: boolean;
    /** The backend takes {@link queryAst}. Default `false`. */
    queryAst: boolean;
    /**
     * The backend has transactions. Default `true`.
     *
     * `false` makes {@link Connection.transaction} refuse with
     * `ERR_DB_UNSUPPORTED` rather than emit a `BEGIN` the backend has never
     * heard of, and makes {@link Connection.executeMany} run its batch without
     * one — so a batch is **not** atomic there, which is why this is declared
     * rather than assumed.
     */
    transactions: boolean;
  }

  /**
   * The half of a connection every backend implements the same way. A driver
   * supplies `_query`, `_execute`, `_close` and a dialect; transactions,
   * savepoints, the closed-connection check and the error shapes come with it.
   */
  export abstract class BaseConnection implements Connection {
    constructor(options: { dialect: Dialect; backend: string });
    readonly dialect: Dialect;
    readonly backend: string;
    /**
     * Whether this connection is still worth using at all.
     *
     * A driver overrides it to account for a transport that died while nobody
     * was looking. A pool checks it before handing a connection out.
     */
    get usable(): boolean;
    /**
     * Whether this connection is fit for the **next** caller — the one question
     * a protocol-blind pool cannot answer for itself, asked by one name on
     * every backend. Defaults to alive and not inside a transaction; a driver
     * adds what its protocol knows.
     */
    get reusable(): boolean;
    query(q: Queryable, params?: DbParams, options?: CallOptions): Promise<Rows>;
    execute(q: Queryable, params?: DbParams, options?: CallOptions): Promise<ExecuteResult>;
    executeMany(q: string | Query, rows: readonly DbParams[]): Promise<ExecuteResult>;
    transaction<T>(fn: (tx: Connection) => Promise<T>): Promise<T>;
    /**
     * Runs `fn` with a connection held for the whole of it — here, this one.
     *
     * It exists on a single connection so that code which must not be spread
     * over two does not have to know whether it was handed a connection or a
     * pool. A driver overrides it only to refuse: a client with no single
     * session to offer (a cluster) says so by name.
     */
    withConnection<T>(fn: (connection: Connection) => Promise<T>): Promise<T>;
    close(): Promise<void>;
    [Symbol.asyncDispose](): Promise<void>;
    /**
     * Ask the backend to abandon whatever this connection is running.
     *
     * Override it with whatever the backend offers — an interrupt flag for an
     * in-process engine, a cancel on a second connection for a wire protocol.
     * Not overriding it means a `signal` still rejects the caller, but the work
     * runs to completion: the promise is abandoned, the statement is not.
     */
    protected _cancel(): Promise<void>;
    /**
     * Runs `work` with a signal attached, for a driver's own entry points.
     *
     * Aborting cancels and then waits, so the connection is left usable, and
     * the rejection carries the signal's `reason` rather than the backend's
     * word for a cancelled statement.
     */
    protected _withSignal<T>(
      signal: AbortSignal | undefined,
      work: () => Promise<T>,
    ): Promise<T>;
    /** Keeps a signal attached to a result set until its rows end. */
    protected _bindSignalToRows(signal: AbortSignal, rows: Rows, onAbort: () => void): Rows;
    /**
     * Throws `ERR_DB_CLOSED` if this connection is closed.
     *
     * For a driver adding methods of its own: every entry point should refuse a
     * closed connection the same way the built-in ones do.
     */
    protected _open(): void;
    protected abstract _query(q: NormalizedQuery): Promise<Rows>;
    protected abstract _execute(q: NormalizedQuery): Promise<ExecuteResult>;
    /**
     * The batch path. **Optional** — the default loops `_execute`, which is
     * correct and no faster than the loop it replaces. Override it with
     * whatever the backend does in one round trip.
     *
     * It takes the whole {@link NormalizedQuery} rather than just its text, so
     * that a backend which took an AST still has one here.
     */
    protected _executeMany(
      query: NormalizedQuery,
      sets: [DbInput[], [string, DbInput][]][],
    ): Promise<ExecuteResult>;
    /**
     * The three statements a transaction is made of.
     *
     * They default to the SQL every SQL backend spells the same way. A backend
     * that does not speak SQL overrides them — `MULTI`/`EXEC`, a protocol
     * message, an engine call — rather than inheriting a `BEGIN` it cannot use.
     *
     * `name` is the savepoint's, and is `null` at the outermost level; only a
     * backend claiming `supports.savepoints` ever sees `nested: true`.
     */
    protected _beginTransaction(scope: TransactionScope): Promise<void>;
    protected _commitTransaction(scope: TransactionScope): Promise<void>;
    protected _rollbackTransaction(scope: TransactionScope): Promise<void>;
    protected abstract _close(): Promise<void>;
  }

  /**
   * A query after the dialect has rendered it.
   *
   * Exactly one of `text` and `ast` is non-null: which one depends on the form
   * the caller used and on what the backend declared it takes.
   */
  export interface NormalizedQuery {
    /** The rendered SQL, or `null` for a backend that took an AST. */
    readonly text: string | null;
    /** The AST {@link queryAst} carried, or `null` for a SQL backend. */
    readonly ast: unknown;
    readonly positional: DbInput[];
    readonly named: [string, DbInput][];
  }

  /** How a {@link Pool} makes, checks, and destroys what it holds. */
  export interface PoolOptions<T> {
    create: () => Promise<T>;
    destroy: (resource: T) => unknown;
    /** Checked before a pooled resource is handed out again. */
    validate?: (resource: T) => boolean | Promise<boolean>;
    /** Most resources to hold at once. Default 10. */
    max?: number;
    /** How long an unused resource is kept, in ms. Default 30 000; `0` never reaps. */
    idleTimeout?: number;
    /** How long to wait for a free resource, in ms. Default 10 000; `0` waits forever. */
    acquireTimeout?: number;
  }

  /**
   * A pool of connections, or of anything else a driver has to make and keep.
   *
   * Protocol-blind: it knows how to make a thing, how to destroy one, and how
   * many to allow. What it cannot know is whether a returned connection is fit
   * to reuse — that needs the protocol, so the driver asserts it on
   * {@link Pool.release}, and anything not explicitly clean is destroyed.
   * Getting that backwards is how an aborted transaction or an open portal
   * leaks from one request into the next.
   *
   * Idle resources are swept **on use, not on a timer**: a repeating timer
   * would keep the event loop alive for as long as the pool existed, so a
   * program that had finished its work would not exit.
   */
  export class Pool<T = unknown> {
    constructor(options: PoolOptions<T>);
    /** Borrowed and idle together. */
    readonly size: number;
    readonly idle: number;
    /** Callers queued behind a full pool. */
    readonly pending: number;
    /** Usable until closed. */
    readonly usable: boolean;
    /** Always true while open: an unfit connection was destroyed, not kept. */
    readonly reusable: boolean;
    acquire(): Promise<T>;
    /**
     * Returns a resource. `clean` is the driver's assertion that it is fit for
     * the next caller, and defaults to **false** — when nobody checked, the
     * safe answer is to throw it away.
     */
    release(resource: T, options?: { clean?: boolean }): void;
    /** Destroys every idle resource and refuses everyone still waiting. */
    close(): Promise<void>;
  }

  /**
   * A backend, as a value.
   *
   * Imported and handed to {@link connect} rather than installed by the side
   * effect of an import. `C` is the connection it opens, `O` the options it
   * takes, `P` its pooled form — which is what makes `connect` return the
   * driver's own connection type rather than the portable minimum.
   */
  export interface Driver<C extends Connection = Connection, O = object, P = PooledConnection> {
    /** The backend's name, e.g. `"postgres"`. Reported as `Connection.backend`. */
    readonly name: string;
    /** The schemes it takes, without colons, e.g. `["postgres", "postgresql"]`. */
    readonly schemes: readonly string[];
    readonly dialect: Dialect;
    /** Whether it takes URLs of that scheme, with or without the colon. */
    accepts(scheme: string): boolean;
    /**
     * Opens one connection. {@link connect} is how callers reach this; a driver
     * calls it directly when it opens connections of its own — a cluster client
     * following a redirect, a pool filling a slot.
     */
    open(url: string, options?: O): Promise<C>;
    /**
     * Opens the pooled form. {@link connect}'s `pool` option is this.
     *
     * May be async — a driver that has to look something up before it can pool
     * (Sentinel asking where the master is) does it here, so a misconfiguration
     * fails at `connect` rather than at the first command.
     */
    pooled(url: string, options?: O, poolOptions?: PoolSettings): P | Promise<P>;
  }

  /** Any driver, for code that holds one without caring what it opens. */
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  export type AnyDriver = Driver<Connection, any, unknown>;

  /** What {@link defineDriver} takes. */
  export interface DriverSpec<C extends Connection, O = object, P = PooledConnection> {
    name: string;
    schemes: readonly string[];
    dialect: Dialect;
    open(url: string, options: O): Promise<C>;
    /**
     * The pooled form, when the default {@link PooledConnection} is not it —
     * a driver adding its own surface to a pool, or refusing to pool a URL that
     * cannot be pooled.
     */
    pooled?(url: string, options: O, poolOptions: PoolSettings): P | Promise<P>;
  }

  /**
   * Defines a driver.
   *
   * ```js
   * export default defineDriver({
   *   name: "mydb",
   *   schemes: ["mydb"],
   *   dialect,
   *   open: (url, options) => MyConnection.connect(url, options),
   * });
   * ```
   */
  export function defineDriver<C extends Connection, O = object, P = PooledConnection>(
    spec: DriverSpec<C, O, P>,
  ): Driver<C, O, P>;

  /**
   * A pool that behaves like one connection.
   *
   * What `connect(url, { driver, pool: true })` returns. It implements the same
   * {@link Connection} surface a single connection does and borrows a real one
   * per call. A driver subclasses it to put its own surface on the pooled form.
   */
  export class PooledConnection implements Connection {
    constructor(driver: AnyDriver, url: string, options?: object, poolOptions?: PoolSettings);
    readonly dialect: Dialect;
    readonly backend: string;
    /** Borrowed and idle together. */
    readonly size: number;
    readonly idle: number;
    /** Callers queued behind a full pool. */
    readonly pending: number;
    query(q: Queryable, params?: DbParams, options?: CallOptions): Promise<Rows>;
    execute(q: Queryable, params?: DbParams, options?: CallOptions): Promise<ExecuteResult>;
    executeMany(q: string | Query, rows: readonly DbParams[]): Promise<ExecuteResult>;
    /** Runs `fn` in a transaction on **one** connection. */
    transaction<T>(fn: (tx: Connection) => Promise<T>): Promise<T>;
    /**
     * Runs `fn` with one connection held for the whole of it — the escape hatch
     * for everything stateful across calls: a session setting, a `LISTEN`, a
     * `WATCH`.
     */
    withConnection<T>(fn: (connection: Connection) => Promise<T>): Promise<T>;
    /** Returns a borrowed connection, destroying it unless `reusable`. */
    protected _release(connection: Connection): void;
    close(): Promise<void>;
    [Symbol.asyncDispose](): Promise<void>;
  }

  /** A growable buffer with the length-prefix helpers every wire protocol needs. */
  export class ByteWriter {
    constructor(capacity?: number);
    readonly length: number;
    u8(value: number): this;
    i16(value: number): this;
    i32(value: number): this;
    i64(value: number | bigint): this;
    f64(value: number): this;
    bytes(value: Uint8Array): this;
    /** Reserves a length written once the body's size is known. */
    beginLength(): number;
    /** Back-fills a reserved length. */
    endLength(at: number, options?: { inclusive?: boolean }): this;
    finish(): Uint8Array;
  }

  /**
   * Decodes one column's span into a JS value.
   *
   * Returns `unknown` rather than {@link DbOutput}: a backend decides what its
   * types become, and a driver that turns `timestamptz` into a `Date` or
   * `jsonb` into an object is doing its job, not exceeding it. {@link DbOutput}
   * describes what the built-in backend produces, not a ceiling.
   */
  export type ColumnDecoder = (
    bytes: Uint8Array,
    view: DataView,
    start: number,
    length: number,
  ) => unknown;

  /** The accessor class for one query's result shape. */
  export interface RowShape {
    new (bytes: Uint8Array, view: DataView, offsets: Int32Array, at: number): Row;
    readonly columns: readonly Column[];
    readonly decoders: readonly ColumnDecoder[];
  }

  /**
   * Builds the accessor class for a result shape — prototype getters, not a
   * `Proxy`, so every row of a query stays on one hidden class. Omit `decoders`
   * for a backend whose values carry their own type tag.
   */
  export function defineRowShape(
    columns: readonly Column[],
    options?: { decoders?: readonly ColumnDecoder[] },
  ): RowShape;

  /** The accessor class for records — values in column order, already decoded. */
  export interface RecordShape {
    new (values: readonly unknown[]): Row;
    readonly columns: readonly Column[];
    readonly records: true;
  }

  /**
   * Builds the accessor class for a backend whose values are **already
   * JavaScript**.
   *
   * The byte layout exists because a wire protocol hands over bytes and
   * decoding them lazily is worth the machinery. A backend that never had bytes
   * — a document store answering JSON, a graph or vector service over HTTP, an
   * in-process engine holding objects — would otherwise encode its values so
   * that `decodeBatch` could immediately take them apart again. Same `Row`
   * contract either way; nothing downstream can tell which kind it holds.
   */
  export function defineRecordShape(columns: readonly Column[]): RecordShape;

  /** Walks a batch and returns its rows, decoding nothing until a column is read. */
  export function decodeBatch(bytes: Uint8Array, shape: RowShape, rowCount: number): Row[];

  /** Encodes parameters into the tagged buffer the host decodes. */
  export function encodeParams(
    positional?: readonly DbInput[],
    named?: readonly [string, DbInput][],
  ): Uint8Array;

  /** Encodes many parameter sets for one statement — a batched execute's payload. */
  export function encodeParamSets(
    sets: readonly [readonly DbInput[], readonly [string, DbInput][]][],
  ): Uint8Array;

  /** Splits a caller's parameters into positional and named. */
  export function splitParams(params?: DbParams): [DbInput[], [string, DbInput][]];

  /** Matches an error against a table of patterns, returning the first portable code. */
  export function mapError(
    e: unknown,
    table: readonly [string | RegExp, string][],
    fallback?: string | null,
  ): string | null;

  /** Rewraps a failure as a {@link DbError}, layering driver code over host code. */
  export function asDbError(e: unknown, code?: string | null): DbError;

  /** Which transaction level `_beginTransaction` and friends are running at. */
  export interface TransactionScope {
    readonly nested: boolean;
    /** The savepoint's name, or `null` at the outermost level. */
    readonly name: string | null;
  }

  /** One conformance check's outcome. */
  export interface ConformanceResult {
    readonly name: string;
    readonly ok?: boolean;
    readonly skipped?: boolean;
    /** Why it was skipped — the caller asked, or the backend cannot express it. */
    readonly reason?: string;
    readonly error?: string;
  }

  /** What {@link runBackendConformance} reports. */
  export interface ConformanceReport {
    readonly ok: boolean;
    readonly passed: number;
    readonly skipped: number;
    readonly failures: ConformanceResult[];
    readonly results: ConformanceResult[];
  }

  /**
   * Runs the conformance suite against a backend, so a driver can demonstrate
   * it behaves like the built-ins rather than intend to. `open` is called once
   * per check and must resolve to a fresh connection; each check builds and
   * drops its own table.
   *
   * Most checks are written in SQL. Against a backend that declares
   * `supports.sqlText: false` those are **skipped with a reason** rather than
   * failed — a check you cannot express is not a finding — and what runs is the
   * part that holds for every backend whatever form it takes.
   */
  export function runBackendConformance(
    open: () => Promise<Connection>,
    options?: { skip?: readonly string[] },
  ): Promise<ConformanceReport>;

  const _default: {
    connect: typeof connect;
    sql: typeof sql;
    queryAst: typeof queryAst;
    sqlite: typeof sqlite;
    defineDriver: typeof defineDriver;
    DbError: typeof DbError;
    DbErrorCode: typeof DbErrorCode;
  };
  export default _default;
}
