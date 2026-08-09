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
  export interface Rows extends AsyncIterable<Row> {
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
    readonly lastInsertRowid: number | null;
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

  /** An open connection. */
  export interface Connection {
    readonly dialect: Dialect;
    /** The backend's name, e.g. `"sqlite"`. */
    readonly backend: string;
    query(q: Queryable, params?: DbParams): Promise<Rows>;
    execute(q: Queryable, params?: DbParams): Promise<ExecuteResult>;
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
    close(): Promise<void>;
    [Symbol.asyncDispose](): Promise<void>;
  }

  /** Options for {@link connect}. */
  export interface ConnectOptions {
    /**
     * Encryption key — hex string or bytes. **Never put a key in the connection
     * string**; one passed as a URL parameter is refused, because a key in a URL
     * ends up in logs, error messages and stack traces.
     */
    key?: string | Uint8Array | ArrayBuffer | ArrayBufferView;
    /** Cipher name; defaults to the backend's. */
    cipher?: string;
    /** Open without the ability to write (`sqlite:`). */
    readOnly?: boolean;
    [option: string]: unknown;
  }

  /**
   * Opens a connection, choosing the backend by the connection string's scheme.
   *
   * `sqlite:` names a file format and a SQL dialect the way `postgres://` names
   * a wire protocol — not an implementation, which may change without the URL
   * changing. `sqlite::memory:` opens a database that exists only in memory and
   * needs no capability; every other `sqlite:` open is scoped by `--allow-read`
   * / `--allow-write` exactly as a file is.
   */
  export function connect(url: string, options?: ConnectOptions): Promise<Connection>;

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
    returning: boolean;
    savepoints: boolean;
    namedParameters: boolean;
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
    query(q: Queryable, params?: DbParams): Promise<Rows>;
    execute(q: Queryable, params?: DbParams): Promise<ExecuteResult>;
    executeMany(q: string | Query, rows: readonly DbParams[]): Promise<ExecuteResult>;
    transaction<T>(fn: (tx: Connection) => Promise<T>): Promise<T>;
    close(): Promise<void>;
    [Symbol.asyncDispose](): Promise<void>;
    protected abstract _query(q: NormalizedQuery): Promise<Rows>;
    protected abstract _execute(q: NormalizedQuery): Promise<ExecuteResult>;
    protected abstract _executeMany(
      text: string,
      sets: [DbInput[], [string, DbInput][]][],
    ): Promise<ExecuteResult>;
    protected abstract _close(): Promise<void>;
  }

  /** A query after the dialect has rendered it. */
  export interface NormalizedQuery {
    readonly text: string;
    readonly positional: DbInput[];
    readonly named: [string, DbInput][];
  }

  /** Registers a backend for a connection-string scheme. */
  export function registerBackend(
    scheme: string,
    factory: (url: string, options: ConnectOptions) => Promise<Connection>,
  ): void;

  /** The registered schemes, in registration order. */
  export function backendSchemes(): string[];

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

  /** Decodes one column's span into a JS value. */
  export type ColumnDecoder = (
    bytes: Uint8Array,
    view: DataView,
    start: number,
    length: number,
  ) => DbOutput;

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

  /** One conformance check's outcome. */
  export interface ConformanceResult {
    readonly name: string;
    readonly ok?: boolean;
    readonly skipped?: boolean;
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
   */
  export function runBackendConformance(
    open: () => Promise<Connection>,
    options?: { skip?: readonly string[] },
  ): Promise<ConformanceReport>;

  const _default: {
    connect: typeof connect;
    sql: typeof sql;
    queryAst: typeof queryAst;
    DbError: typeof DbError;
    DbErrorCode: typeof DbErrorCode;
    registerBackend: typeof registerBackend;
  };
  export default _default;
}
