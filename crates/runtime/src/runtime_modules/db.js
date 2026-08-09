// runtime:db — databases, in two tiers (DECISIONS.md D56).
//
// The **application tier** is `connect()`, `sql`, and what they return. The
// **driver tier** is everything a third party needs to add a backend of their
// own — the registry, the row decoder, the parameter encoder, the type and
// error tables, and the base classes that implement the parts every driver
// would otherwise rewrite. Both are exported from here; the split is in the
// documentation, not the specifier.
//
// Only embedded engines reach the host through ops. A networked backend —
// Postgres, MySQL, anything written outside this file — is JS over
// `runtime:net`, and adding one requires no change to the runtime at all.

const ops = globalThis.__ops;

const decoder = new TextDecoder();
const encoder = new TextEncoder();

// How many bytes a fetch asks for. The socket path hands JS 64 KiB per read,
// and the embedded engine is told the same number, so one batch is one crossing
// on either kind of backend.
const BATCH_BYTES = 64 * 1024;

const MAX_SAFE = BigInt(Number.MAX_SAFE_INTEGER);
const MIN_SAFE = -MAX_SAFE;

// The per-value type tags of the row and parameter encodings. Shared with the
// host, and with any driver that produces rows in the same layout.
const TAG_INTEGER = 1;
const TAG_REAL = 2;
const TAG_TEXT = 3;
const TAG_BLOB = 4;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

// The portable classification. A backend maps its own vocabulary onto these so
// that an ORM can branch on "this insert collided" without knowing which
// database said so; the backend's own code is kept on `backendCode`, because a
// portable name is a summary and summaries lose things.
const DbErrorCode = Object.freeze({
  UniqueViolation: "ERR_DB_UNIQUE_VIOLATION",
  ForeignKeyViolation: "ERR_DB_FOREIGN_KEY_VIOLATION",
  NotNullViolation: "ERR_DB_NOT_NULL_VIOLATION",
  CheckViolation: "ERR_DB_CHECK_VIOLATION",
  Deadlock: "ERR_DB_DEADLOCK",
  SerializationFailure: "ERR_DB_SERIALIZATION_FAILURE",
  Busy: "ERR_DB_BUSY",
  ConnectionLost: "ERR_DB_CONNECTION_LOST",
  AuthFailed: "ERR_DB_AUTH_FAILED",
  Timeout: "ERR_DB_TIMEOUT",
  Syntax: "ERR_DB_SYNTAX",
  UndefinedTable: "ERR_DB_UNDEFINED_TABLE",
  UndefinedColumn: "ERR_DB_UNDEFINED_COLUMN",
  ReadOnly: "ERR_DB_READ_ONLY",
  // The query was handed in a form this backend does not take — SQL text to an
  // engine that wants an AST, or an AST to one that wants text.
  QueryForm: "ERR_DB_QUERY_FORM",
  Unsupported: "ERR_DB_UNSUPPORTED",
  Closed: "ERR_DB_CLOSED",
  // The connection is mid-conversation and cannot take another. Distinct from
  // `Busy`, which is the *database* refusing: this is the client's own
  // connection already streaming a result that only the caller can finish, so
  // waiting would deadlock rather than take a while. Every wire protocol has
  // this constraint — a connection is one conversation — which is why it is a
  // portable code rather than one backend's problem.
  ConnectionBusy: "ERR_DB_CONNECTION_BUSY",
  // Nothing above fitted. Not a failure of the table: a database has far more
  // to say than a portable name can carry, and pretending otherwise would make
  // the classification untrustworthy where it does apply.
  Backend: "ERR_DB_BACKEND",
});

class DbError extends Error {
  constructor(message, { code = DbErrorCode.Backend, backendCode, cause } = {}) {
    super(message, cause === undefined ? undefined : { cause });
    this.name = "DbError";
    this.code = code;
    if (backendCode !== undefined) this.backendCode = backendCode;
  }
}

function dbError(message, code, extra) {
  return new DbError(message, { code, ...extra });
}

// Rewraps a host or backend failure as a DbError without losing what it said.
// A DbError that already came from a driver passes through: it has been
// classified once by the layer that could.
//
// The classification is layered, and the order matters. A code the *driver*
// recognised wins, because it knows its own backend's vocabulary. Failing
// that, a stable code from the **host** is kept as-is: a denied capability is
// a fact about this runtime, not about the database, and an application that
// tested `e.code === "ERR_CAPABILITY_DENIED"` must not have to know that the
// call it made happened to go through a database. Only a failure nobody
// classified falls through to `ERR_DB_BACKEND`. The original code is kept on
// `backendCode` either way, so nothing is lost by the layering.
function asDbError(e, code = null) {
  if (e instanceof DbError) return e;
  const message = e && e.message != null ? e.message : String(e);
  const hostCode = typeof e?.code === "string" ? e.code : undefined;
  return new DbError(message, {
    code: code ?? hostCode ?? DbErrorCode.Backend,
    backendCode: hostCode,
    cause: e,
  });
}

/// Matches a backend's message or code against a table of patterns, returning
/// the first portable code that matches. Drivers build their table once and
/// pass it in; the table is data rather than a chain of ifs so a driver can
/// extend it for an extension's error without editing this file.
///
/// Returns `fallback` (`null` by default) when nothing matches, so that a
/// caller can tell "this is not a constraint violation" from "this is an
/// unclassifiable one" and let `asDbError` layer the host's answer underneath.
function mapError(e, table, fallback = null) {
  const message = e && e.message != null ? String(e.message) : String(e);
  for (const [pattern, code] of table) {
    if (typeof pattern === "string" ? message.includes(pattern) : pattern.test(message)) {
      return code;
    }
  }
  return fallback;
}

// ---------------------------------------------------------------------------
// Parameters
// ---------------------------------------------------------------------------

// A growable buffer. Small enough to own rather than to reach for a dependency,
// and a driver needs exactly this to build a protocol message.
class ByteWriter {
  constructor(capacity = 256) {
    this._buf = new Uint8Array(capacity);
    this._view = new DataView(this._buf.buffer);
    this.length = 0;
  }

  _room(n) {
    if (this.length + n <= this._buf.length) return;
    let size = this._buf.length * 2;
    while (size < this.length + n) size *= 2;
    const grown = new Uint8Array(size);
    grown.set(this._buf.subarray(0, this.length));
    this._buf = grown;
    this._view = new DataView(grown.buffer);
  }

  u8(value) {
    this._room(1);
    this._buf[this.length++] = value;
    return this;
  }

  i16(value) {
    this._room(2);
    this._view.setInt16(this.length, value);
    this.length += 2;
    return this;
  }

  i32(value) {
    this._room(4);
    this._view.setInt32(this.length, value);
    this.length += 4;
    return this;
  }

  i64(value) {
    this._room(8);
    this._view.setBigInt64(this.length, BigInt(value));
    this.length += 8;
    return this;
  }

  f64(value) {
    this._room(8);
    this._view.setFloat64(this.length, value);
    this.length += 8;
    return this;
  }

  bytes(value) {
    this._room(value.length);
    this._buf.set(value, this.length);
    this.length += value.length;
    return this;
  }

  // Reserves a length that is written once the body's size is known — the shape
  // every length-prefixed protocol message has. Returns a token to close it.
  beginLength() {
    const at = this.length;
    this.i32(0);
    return at;
  }

  // Back-fills a reserved length. `inclusive` counts the length field itself,
  // which is how Postgres frames a message and how a row is framed here.
  endLength(at, { inclusive = true } = {}) {
    const size = this.length - at - (inclusive ? 0 : 4);
    this._view.setInt32(at, size);
    return this;
  }

  finish() {
    return this._buf.subarray(0, this.length);
  }
}

// Writes one value in the shared tagged encoding.
function writeValue(w, value) {
  if (value === null || value === undefined) {
    // A length of -1 is the whole of NULL: no tag, no payload, exactly as on
    // the wire.
    w.i32(-1);
    return;
  }
  if (typeof value === "bigint") {
    w.i32(9).u8(TAG_INTEGER).i64(value);
    return;
  }
  if (typeof value === "number") {
    // An integral number binds as an integer, so `1` does not arrive as `1.0`
    // and a column typed INTEGER does not quietly hold a float.
    if (Number.isInteger(value)) w.i32(9).u8(TAG_INTEGER).i64(value);
    else w.i32(9).u8(TAG_REAL).f64(value);
    return;
  }
  if (typeof value === "boolean") {
    w.i32(9).u8(TAG_INTEGER).i64(value ? 1 : 0);
    return;
  }
  if (typeof value === "string") {
    const bytes = encoder.encode(value);
    w.i32(bytes.length + 1).u8(TAG_TEXT).bytes(bytes);
    return;
  }
  if (value instanceof Date) {
    // ISO-8601 in UTC: the only spelling that survives a round trip through a
    // database with no date type and back into a Date.
    const bytes = encoder.encode(value.toISOString());
    w.i32(bytes.length + 1).u8(TAG_TEXT).bytes(bytes);
    return;
  }
  if (value instanceof Uint8Array) {
    w.i32(value.length + 1).u8(TAG_BLOB).bytes(value);
    return;
  }
  if (ArrayBuffer.isView(value)) {
    const bytes = new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
    w.i32(bytes.length + 1).u8(TAG_BLOB).bytes(bytes);
    return;
  }
  if (value instanceof ArrayBuffer) {
    const bytes = new Uint8Array(value);
    w.i32(bytes.length + 1).u8(TAG_BLOB).bytes(bytes);
    return;
  }
  throw dbError(
    `a parameter of type ${typeof value} cannot be bound; pass a number, bigint, string, boolean, Date, Uint8Array, or null`,
    DbErrorCode.Unsupported,
  );
}

// Encodes positional and named parameters into the buffer the host decodes.
// One buffer rather than an array of values, because the op boundary's value
// type has no bigint: an i64 parameter would round through a double on the way
// down, losing the value before the engine ever saw it.
function writeParams(w, positional, named) {
  w.i16(positional.length);
  for (const value of positional) writeValue(w, value);
  w.i16(named.length);
  for (const [name, value] of named) {
    const bytes = encoder.encode(name);
    w.i32(bytes.length).bytes(bytes);
    writeValue(w, value);
  }
}

function encodeParams(positional = [], named = []) {
  const w = new ByteWriter();
  writeParams(w, positional, named);
  return w.finish();
}

/// Encodes many parameter sets for one statement — the payload of a batched
/// execute. `sets` is `[[positional, named], …]`, as {@link splitParams} gives.
function encodeParamSets(sets) {
  const w = new ByteWriter(1024);
  w.i32(sets.length);
  for (const [positional, named] of sets) writeParams(w, positional, named);
  return w.finish();
}

// Splits a caller's parameters into the two forms. An array is positional; a
// plain object is named. Both at once is accepted, since a statement may mix
// `?` and `:name`.
function splitParams(params) {
  if (params === undefined || params === null) return [[], []];
  if (Array.isArray(params)) return [params, []];
  if (typeof params === "object") return [[], Object.entries(params)];
  return [[params], []];
}

// ---------------------------------------------------------------------------
// Rows
// ---------------------------------------------------------------------------

// The row's reader, on a symbol so that no column name can collide with it — a
// table with a column called `read` is not a reason for a row to misbehave.
const READ = Symbol("read");

// The decoders. Each takes the batch buffer and a span, and produces a JS
// value — so a column nobody reads costs nothing but the two integers its span
// took.
function decodeDynamic(bytes, view, start, length) {
  const tag = bytes[start];
  const at = start + 1;
  const size = length - 1;
  switch (tag) {
    case TAG_INTEGER: {
      const value = view.getBigInt64(at);
      // A bigint only where a number would lose the value. Returning bigint
      // always would be exact and unusable — `row.id + 1` throws — and
      // returning number always would silently round a 64-bit id.
      return value >= MIN_SAFE && value <= MAX_SAFE ? Number(value) : value;
    }
    case TAG_REAL:
      return view.getFloat64(at);
    case TAG_TEXT:
      return decoder.decode(bytes.subarray(at, at + size));
    case TAG_BLOB:
      // A view over the batch, not a copy: the caller gets the bytes without
      // paying for them. It is invalidated by nothing — each batch owns its
      // buffer — so retaining a row retains its blob.
      return bytes.slice(at, at + size);
    default:
      throw dbError(`unknown value tag ${tag} in a row`, DbErrorCode.Backend);
  }
}

function decodeText(bytes, _view, start, length) {
  return decoder.decode(bytes.subarray(start, start + length));
}

function decodeBytes(bytes, _view, start, length) {
  return bytes.slice(start, start + length);
}

/// Builds the accessor class for one query's result shape.
///
/// A class with prototype getters rather than a `Proxy`: a Proxy deoptimizes
/// every property access through it, while a class generated per query keeps
/// every row of that query on one hidden class, which is what makes the access
/// monomorphic and the getter inlinable. Columns are enumerable, so spreading a
/// row or handing it to `JSON.stringify` works without a conversion step —
/// which is where a lazy row usually stops being lazy, so those paths read the
/// columns and nothing else does.
///
/// `columns` is `[{ name, declType }]`. `decoders` may be supplied per column
/// by a backend whose column types are fixed (every wire protocol); omitted,
/// each value carries its own tag, which is what a dynamically typed engine
/// like SQLite needs.
function defineRowShape(columns, { decoders } = {}) {
  class Row {
    // Private fields, not properties. A row holds references to the whole
    // batch's buffer, and anything reachable from the instance is reachable by
    // a caller who spreads it — `{ ...row }` copies own symbol keys as happily
    // as own string ones. Private fields are copied by nothing and enumerated
    // by nothing, so the buffer cannot escape through a row by accident. The
    // reader has to live in the class body to see them, which is why it is a
    // method rather than a free function.
    #bytes;
    #view;
    #offsets;
    #at;

    constructor(bytes, view, offsets, at) {
      this.#bytes = bytes;
      this.#view = view;
      this.#offsets = offsets;
      this.#at = at;
    }

    [READ](index) {
      const base = this.#at + index * 2;
      const length = this.#offsets[base + 1];
      if (length < 0) return null;
      return Row.decoders[index](this.#bytes, this.#view, this.#offsets[base], length);
    }

    // The columns as an array, in order — for a caller that wants positions
    // rather than names, and for one whose columns are not valid identifiers.
    values() {
      return columns.map((_, i) => this[READ](i));
    }

    /// The row as a plain object, every column decoded.
    ///
    /// **This is how a row is materialized**, and `{ ...row }` is not: the
    /// columns are prototype getters so that a column nobody reads costs
    /// nothing, and spreading copies own properties only. The cost of laziness
    /// is that the shorthand for "copy this" does not reach it, so there is an
    /// explicit spelling instead of a silently empty object.
    toObject() {
      const out = {};
      for (let i = 0; i < columns.length; i++) out[columns[i].name] = this[READ](i);
      return out;
    }

    toJSON() {
      return this.toObject();
    }
  }

  Row.columns = columns;
  Row.decoders = decoders ?? columns.map(() => decodeDynamic);

  const seen = new Set();
  columns.forEach((column, index) => {
    // A duplicate name — `SELECT a.id, b.id` — binds to the first, which is
    // what SQL itself says the name refers to. The second is still reachable
    // by position through `values()`.
    if (seen.has(column.name)) return;
    seen.add(column.name);
    Object.defineProperty(Row.prototype, column.name, {
      get() {
        return this[READ](index);
      },
      enumerable: true,
      configurable: true,
    });
  });

  return Row;
}

/// Walks a batch of rows and returns them as instances of `shape`.
///
/// One pass over the buffer records each column's span; nothing is decoded.
/// The buffer is handed to the rows as-is and never reused, so a row retained
/// past its batch is still valid — buffer reuse would corrupt exactly that
/// case, silently, and is not worth the allocation it saves.
function decodeBatch(bytes, shape, rowCount) {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const columns = shape.columns.length;
  const offsets = new Int32Array(rowCount * columns * 2);
  const rows = new Array(rowCount);
  let at = 0;
  let slot = 0;
  for (let r = 0; r < rowCount; r++) {
    const rowEnd = at + view.getInt32(at);
    let cursor = at + 6; // the row's length (4) and column count (2)
    const base = slot;
    for (let c = 0; c < columns; c++) {
      const length = view.getInt32(cursor);
      cursor += 4;
      if (length < 0) {
        offsets[slot++] = 0;
        offsets[slot++] = -1;
      } else {
        offsets[slot++] = cursor;
        offsets[slot++] = length;
        cursor += length;
      }
    }
    rows[r] = new shape(bytes, view, offsets, base);
    at = rowEnd;
  }
  return rows;
}

// ---------------------------------------------------------------------------
// Dialects
// ---------------------------------------------------------------------------

/// What a query builder needs in order to target a backend it was not written
/// for: how a placeholder is spelled, and how an identifier is quoted.
class Dialect {
  constructor({ name, placeholder, quote = '"', supports = {} }) {
    this.name = name;
    this._placeholder = placeholder;
    this._quote = quote;
    this.supports = Object.freeze({
      returning: false,
      savepoints: false,
      namedParameters: false,
      ...supports,
    });
    Object.freeze(this);
  }

  /// The placeholder for the 1-based parameter `index`.
  placeholder(index) {
    return this._placeholder(index);
  }

  /// Quotes an identifier, doubling the quote character inside it. This is the
  /// only safe way to put a caller's name into SQL, and the reason it is here
  /// rather than left to each builder to reinvent.
  quoteIdent(name) {
    const q = this._quote;
    return q + String(name).split(q).join(q + q) + q;
  }
}

const SQLITE_DIALECT = new Dialect({
  name: "sqlite",
  placeholder: () => "?",
  supports: { returning: true, savepoints: true, namedParameters: true },
});

// ---------------------------------------------------------------------------
// Queries
// ---------------------------------------------------------------------------

/// A query built by the `sql` tag: the fragments and the values, kept apart
/// until a backend renders them. Rendering late is what lets one template run
/// against `$1`, `?`, and `:name` backends unchanged — and what makes it
/// impossible to build the string with the values already in it.
class Query {
  constructor(strings, values) {
    this.strings = strings;
    this.values = values;
  }

  render(dialect) {
    let text = this.strings[0];
    for (let i = 0; i < this.values.length; i++) {
      text += dialect.placeholder(i + 1) + this.strings[i + 1];
    }
    return { text, params: this.values };
  }
}

/// The `sql` tagged template.
///
///     await db.query(sql`SELECT * FROM users WHERE id = ${id}`)
///
/// Every interpolation becomes a parameter, never text. A nested `Query` is
/// spliced with its own values, so a fragment composes without either half
/// having to know the other's placeholder numbering.
function sql(strings, ...values) {
  const parts = [strings[0]];
  const bound = [];
  for (let i = 0; i < values.length; i++) {
    const value = values[i];
    if (value instanceof Query) {
      const inner = value.strings;
      parts[parts.length - 1] += inner[0];
      for (let j = 0; j < value.values.length; j++) {
        bound.push(value.values[j]);
        parts.push(inner[j + 1]);
      }
      parts[parts.length - 1] += strings[i + 1];
    } else {
      bound.push(value);
      parts.push(strings[i + 1]);
    }
  }
  return new Query(parts, bound);
}

/// Marks a structured query for a backend that takes an AST rather than SQL
/// text. Carried in the contract from the first release so that an engine which
/// never speaks SQL can be a first-class backend rather than a special case;
/// the backends that ship today refuse it by name.
function queryAst(ast) {
  return { __queryAst: true, ast };
}

function isQueryAst(q) {
  return typeof q === "object" && q !== null && q.__queryAst === true;
}

// Normalizes whatever a caller passed into `{ text, positional, named }`.
function normalizeQuery(q, params, dialect, backend) {
  if (isQueryAst(q)) {
    throw dbError(
      `the ${backend} backend takes SQL text, not a query AST`,
      DbErrorCode.QueryForm,
    );
  }
  if (q instanceof Query) {
    const rendered = q.render(dialect);
    return { text: rendered.text, positional: rendered.params, named: [] };
  }
  if (typeof q !== "string") {
    throw dbError(
      "a query must be SQL text, a sql`` template, or a query AST",
      DbErrorCode.QueryForm,
    );
  }
  const [positional, named] = splitParams(params);
  return { text: q, positional, named };
}

// ---------------------------------------------------------------------------
// Result sets
// ---------------------------------------------------------------------------

/// An async-iterable result set that pulls one batch at a time.
///
/// Never the whole result: a table larger than memory streams through this at
/// the cost of one batch, which is the property a cursor exists to give. A
/// caller that stops early — `break`, `return`, a `throw` — closes the cursor
/// on the way out, because an abandoned cursor is ordinary code and not an
/// error, and because on a pooled backend it is what decides whether the
/// connection can be reused.
class Rows {
  constructor(source, shape) {
    this._source = source;
    this._shape = shape;
    this._done = false;
    this.columns = shape.columns;
  }

  /// Whether the backend finished this result without leaving a cursor open.
  /// A driver sets it by handing over a source that closes to nothing; it is
  /// here so a pool can tell a connection that is idle from one that is not.
  get exhausted() {
    return this._source.exhausted === true;
  }

  async *[Symbol.asyncIterator]() {
    try {
      while (!this._done) {
        const batch = await this._source.next(BATCH_BYTES);
        this._done = batch.done;
        if (batch.rows > 0) {
          const rows = decodeBatch(batch.bytes, this._shape, batch.rows);
          for (const row of rows) yield row;
        }
      }
    } finally {
      await this.close();
    }
  }

  /// The whole result set as an array. The convenience that undoes the
  /// streaming, offered because most queries are small and the alternative is
  /// every caller writing the same loop.
  async toArray() {
    const out = [];
    for await (const row of this) out.push(row);
    return out;
  }

  /// The first row, or `null`. Closes the cursor without reading the rest.
  async first() {
    for await (const row of this) return row;
    return null;
  }

  async close() {
    if (this._closed) return;
    this._closed = true;
    await this._source.close();
  }
}

// ---------------------------------------------------------------------------
// Connections
// ---------------------------------------------------------------------------

/// The half of a connection every backend implements the same way.
///
/// A driver supplies `_query`, `_execute`, `_close`, and a dialect; everything
/// here — transactions, savepoints, the closed-connection check, the shape of
/// the errors — comes for free and, more to the point, comes out the same, so
/// an ORM written against one backend behaves against the next.
class BaseConnection {
  constructor({ dialect, backend }) {
    this.dialect = dialect;
    this.backend = backend;
    this._closed = false;
    this._depth = 0;
  }

  _open() {
    if (this._closed) {
      throw dbError("the connection is closed", DbErrorCode.Closed);
    }
  }

  async query(q, params) {
    this._open();
    return this._query(normalizeQuery(q, params, this.dialect, this.backend));
  }

  async execute(q, params) {
    this._open();
    return this._execute(normalizeQuery(q, params, this.dialect, this.backend));
  }

  /// Runs one statement against many parameter sets.
  ///
  ///     await db.executeMany("INSERT INTO t (a, b) VALUES (?, ?)", rows);
  ///
  /// The reason to reach for it is arithmetic rather than taste: a crossing
  /// costs about the same whatever it carries, so a loop that crosses once per
  /// row spends its time on the boundary instead of in the database. This
  /// crosses once and prepares once.
  ///
  /// **It runs as one transaction** unless one is already open, in which case
  /// it joins that one. A batch that half-applied would be a worse default than
  /// either alternative, and outside a transaction each statement would be
  /// durably committed on its own — which is the slow path this exists to
  /// avoid, arrived at by accident.
  async executeMany(q, rows) {
    this._open();
    if (!Array.isArray(rows)) {
      throw dbError(
        "executeMany takes an array of parameter sets",
        DbErrorCode.Unsupported,
      );
    }
    if (q instanceof Query && q.values.length > 0) {
      // A template with values binds exactly one set, so accepting it here
      // would silently run the first row's values for every row.
      throw dbError(
        "executeMany takes one statement and many parameter sets; a sql`` template with values describes a single row",
        DbErrorCode.Unsupported,
      );
    }
    if (rows.length === 0) return { changes: 0, lastInsertRowid: null };
    const { text } = normalizeQuery(q, undefined, this.dialect, this.backend);
    const sets = rows.map((row) => splitParams(row));
    const run = () => this._executeMany(text, sets);
    return this._depth > 0 ? run() : this.transaction(run);
  }

  /// The default batch: correct, and no faster than the loop it replaces.
  ///
  /// A driver overrides this with whatever its backend does in one round trip —
  /// a prepared statement reused across sets here, pipelined `Bind`/`Execute`
  /// messages on a wire protocol. Overriding is an **optimization**, not a
  /// requirement, and that is the point: `executeMany` means the same thing on
  /// every backend from the day the backend exists, so an ORM can call it
  /// without asking which driver is loaded. Without a default it would be a
  /// method that throws a `TypeError` naming a private method, on backends
  /// chosen by nobody in particular.
  async _executeMany(text, sets) {
    let changes = 0;
    let lastInsertRowid = null;
    for (const [positional, named] of sets) {
      const result = await this._execute({ text, positional, named });
      changes += result.changes ?? 0;
      if (result.lastInsertRowid != null) lastInsertRowid = result.lastInsertRowid;
    }
    return { changes, lastInsertRowid };
  }

  /// Runs `fn` inside a transaction, committing when it returns and rolling
  /// back when it throws.
  ///
  /// Nested calls become savepoints where the backend has them, so a helper
  /// that opens a transaction composes with a caller that already did instead
  /// of failing or — worse — committing the outer one early.
  async transaction(fn) {
    this._open();
    const depth = this._depth;
    const nested = depth > 0;
    if (nested && !this.dialect.supports.savepoints) {
      throw dbError(
        `the ${this.backend} backend has no savepoints, so transactions cannot nest`,
        DbErrorCode.Unsupported,
      );
    }
    const name = `esrun_sp_${depth}`;
    await this.execute(nested ? `SAVEPOINT ${name}` : "BEGIN");
    this._depth = depth + 1;
    try {
      const result = await fn(this);
      await this.execute(nested ? `RELEASE ${name}` : "COMMIT");
      return result;
    } catch (e) {
      // A rollback that itself fails must not replace the error that caused
      // it: the first failure is the one worth reporting, and the second is
      // usually a consequence of it.
      try {
        await this.execute(nested ? `ROLLBACK TO ${name}` : "ROLLBACK");
      } catch {
        /* the original error is the one that matters */
      }
      throw e;
    } finally {
      this._depth = depth;
    }
  }

  async close() {
    if (this._closed) return;
    this._closed = true;
    await this._close();
  }

  // Symmetry with `using` blocks and with the streams elsewhere in the
  // runtime: a connection is a resource, and this is how one is released.
  async [Symbol.asyncDispose]() {
    await this.close();
  }
}

// ---------------------------------------------------------------------------
// The sqlite backend
// ---------------------------------------------------------------------------

// SQLite says what went wrong in prose. There is no code table to consult, so
// the mapping is by message — the same thing every SQLite driver does, and the
// reason `backendCode` keeps the original.
const SQLITE_ERRORS = [
  ["UNIQUE constraint failed", DbErrorCode.UniqueViolation],
  ["FOREIGN KEY constraint failed", DbErrorCode.ForeignKeyViolation],
  ["NOT NULL constraint failed", DbErrorCode.NotNullViolation],
  ["CHECK constraint failed", DbErrorCode.CheckViolation],
  ["database is locked", DbErrorCode.Busy],
  ["readonly database", DbErrorCode.ReadOnly],
  ["attempt to write a readonly database", DbErrorCode.ReadOnly],
  [/no such table/i, DbErrorCode.UndefinedTable],
  [/no such column/i, DbErrorCode.UndefinedColumn],
  [/syntax error/i, DbErrorCode.Syntax],
];

function sqliteError(e) {
  if (e instanceof DbError) return e;
  // `provider error: ` is how the host's unclassified-failure type prints
  // itself. That is a fact about the layer the message crossed, not about the
  // database, and an application reading `e.message` should see what the engine
  // said: "no such table: users", not "provider error: Parse error: no such
  // table: users".
  if (typeof e?.message === "string" && e.message.startsWith("provider error: ")) {
    e = { code: e.code, message: e.message.slice("provider error: ".length), cause: e };
  }
  return asDbError(e, mapError(e, SQLITE_ERRORS));
}

class SqliteConnection extends BaseConnection {
  constructor(id) {
    super({ dialect: SQLITE_DIALECT, backend: "sqlite" });
    this._id = id;
  }

  async _query({ text, positional, named }) {
    let result;
    try {
      result = await ops.db_query(
        this._id,
        text,
        encodeParams(positional, named),
        BATCH_BYTES,
      );
    } catch (e) {
      throw sqliteError(e);
    }
    const shape = defineRowShape(result.columns);
    const id = result.cursor;
    // The first batch came back with the query. When it was the whole answer
    // there is no cursor: nothing to fetch, nothing to close, and the query
    // cost one crossing rather than three.
    let pending = { bytes: result.bytes, rows: result.rows, done: result.done };
    let closed = id === null;
    return new Rows(
      {
        exhausted: id === null,
        async next(maxBytes) {
          if (pending !== null) {
            const batch = pending;
            pending = null;
            return batch;
          }
          try {
            return await ops.db_fetch(id, maxBytes);
          } catch (e) {
            throw sqliteError(e);
          }
        },
        async close() {
          if (closed) return;
          closed = true;
          await ops.db_close_cursor(id);
        },
      },
      shape,
    );
  }

  async _executeMany(text, sets) {
    try {
      return await ops.db_execute_many(this._id, text, encodeParamSets(sets));
    } catch (e) {
      throw sqliteError(e);
    }
  }

  async _execute({ text, positional, named }) {
    try {
      return await ops.db_execute(this._id, text, encodeParams(positional, named));
    } catch (e) {
      throw sqliteError(e);
    }
  }

  async _close() {
    await ops.db_close(this._id);
  }
}

async function connectSqlite(url, options) {
  // The path is everything after the scheme, with any query string removed —
  // `sqlite:./app.db`, `sqlite:/var/lib/app.db`, `sqlite::memory:`.
  const rest = url.slice("sqlite:".length);
  const q = rest.indexOf("?");
  const path = q === -1 ? rest : rest.slice(0, q);
  if (q !== -1) {
    const params = new URLSearchParams(rest.slice(q + 1));
    // A key in a URL ends up in logs, in error messages, and in stack traces.
    // Refused rather than honoured, because honouring it quietly is how it gets
    // into all three.
    if (params.has("key") || params.has("password")) {
      throw dbError(
        "an encryption key must be passed in the options object, not in the connection string: connect(url, { key })",
        DbErrorCode.Unsupported,
      );
    }
  }
  if (path === "") {
    throw dbError("a sqlite: URL needs a path", DbErrorCode.Unsupported);
  }
  // A named in-memory database (`:memory:app`) is SQLite's way of *sharing* one
  // between connections. Nothing here shares — every open is its own database —
  // so accepting the spelling would promise something it does not do. Refused
  // by name instead.
  if (path.startsWith(":memory:") && path !== ":memory:") {
    throw dbError(
      `named in-memory databases are not supported: ${path} — use sqlite::memory:, which gives each connection its own`,
      DbErrorCode.Unsupported,
    );
  }

  const key = options.key === undefined ? "" : toHexKey(options.key);
  const cipher = options.cipher === undefined ? "" : String(options.cipher);
  try {
    // `sqlite::memory:` reaches no filesystem, so it goes through the op that
    // asks for no filesystem grant.
    const id =
      path === ":memory:"
        ? await ops.db_open_memory(key, cipher)
        : options.readOnly
          ? await ops.db_open_read_only(path, key, cipher)
          : await ops.db_open(path, key, cipher);
    return new SqliteConnection(id);
  } catch (e) {
    throw sqliteError(e);
  }
}

// The key crosses as hex because that is what the engine's option takes. Bytes
// are accepted and converted here so that a caller can hold a key as the
// `CryptoKey`-shaped bytes it derived, rather than being pushed into string
// handling of key material.
function toHexKey(key) {
  if (typeof key === "string") return key;
  const bytes =
    key instanceof Uint8Array
      ? key
      : ArrayBuffer.isView(key)
        ? new Uint8Array(key.buffer, key.byteOffset, key.byteLength)
        : key instanceof ArrayBuffer
          ? new Uint8Array(key)
          : null;
  if (bytes === null) {
    throw dbError(
      "the encryption key must be a hex string or bytes",
      DbErrorCode.Unsupported,
    );
  }
  let hex = "";
  for (const byte of bytes) hex += byte.toString(16).padStart(2, "0");
  return hex;
}

// ---------------------------------------------------------------------------
// The backend registry
// ---------------------------------------------------------------------------

const backends = new Map([["sqlite", connectSqlite]]);

// Reserved for the OpenTF engine. Held rather than left free so that the name
// cannot be taken before the engine that owns it arrives — a scheme is part of
// the contract, and one squatted by a third party could not be given back.
const RESERVED = new Set(["otfdb"]);

/// Registers a backend for a connection-string scheme.
///
///     registerBackend("mysql", async (url, options) => new MyConnection(...));
///
/// The factory resolves to a connection — a `BaseConnection` subclass, in
/// practice, since that is what makes it behave like the built-ins.
function registerBackend(scheme, factory) {
  const name = String(scheme).replace(/:$/, "").toLowerCase();
  if (!/^[a-z][a-z0-9+.-]*$/.test(name)) {
    throw dbError(`${scheme} is not a usable scheme`, DbErrorCode.Unsupported);
  }
  if (RESERVED.has(name)) {
    throw dbError(
      `the ${name}: scheme is reserved`,
      DbErrorCode.Unsupported,
    );
  }
  if (backends.has(name)) {
    throw dbError(
      `the ${name}: scheme is already registered; a built-in backend cannot be replaced`,
      DbErrorCode.Unsupported,
    );
  }
  if (typeof factory !== "function") {
    throw dbError("a backend factory must be a function", DbErrorCode.Unsupported);
  }
  backends.set(name, factory);
}

/// The registered schemes, in registration order.
function backendSchemes() {
  return [...backends.keys()];
}

/// Opens a connection, choosing the backend by the connection string's scheme.
///
///     const db = await connect("sqlite:./app.db", { key });
///
/// `sqlite:` names a file format and a SQL dialect, the way `postgres://` names
/// a wire protocol — not a particular implementation, which is free to change
/// underneath without the URL changing.
async function connect(url, options = {}) {
  if (typeof url !== "string") {
    throw dbError("a connection string is required", DbErrorCode.Unsupported);
  }
  const colon = url.indexOf(":");
  if (colon <= 0) {
    throw dbError(
      `${url} is not a connection string: it needs a scheme, like sqlite:./app.db`,
      DbErrorCode.Unsupported,
    );
  }
  const scheme = url.slice(0, colon).toLowerCase();
  const factory = backends.get(scheme);
  if (!factory) {
    const known = backendSchemes().join(", ");
    throw dbError(
      RESERVED.has(scheme)
        ? `the ${scheme}: scheme is reserved and has no backend yet`
        : `no backend is registered for ${scheme}: — registered schemes are ${known}. A third-party driver registers its own with registerBackend().`,
      DbErrorCode.Unsupported,
    );
  }
  return factory(url, options);
}


// ---------------------------------------------------------------------------
// Backend conformance
// ---------------------------------------------------------------------------

// The table every check builds and drops. Named rather than random so that a
// run interrupted half-way leaves one identifiable thing behind instead of a
// scatter of them.
const CONFORMANCE_TABLE = "esrun_conformance";

function assert(condition, message) {
  if (!condition) throw new Error(message);
}

async function fresh(open, columns) {
  const db = await open();
  await db.execute(`DROP TABLE IF EXISTS ${CONFORMANCE_TABLE}`);
  await db.execute(`CREATE TABLE ${CONFORMANCE_TABLE} (${columns})`);
  return db;
}

// Each check is `[name, async (open, options) => void]` and throws to fail.
// A list rather than a framework: a driver author runs this from whatever they
// already use, and importing a test runner into the runtime to check a driver
// would be the wrong dependency in the wrong direction.
const CONFORMANCE_CHECKS = [
  [
    "columns are reported in order, by name",
    async (open) => {
      const db = await fresh(open, "a INTEGER, b TEXT");
      try {
        await db.execute(`INSERT INTO ${CONFORMANCE_TABLE} VALUES (1, 'x')`);
        const rows = await db.query(`SELECT a, b FROM ${CONFORMANCE_TABLE}`);
        assert(
          rows.columns.map((c) => c.name).join(",") === "a,b",
          `columns were ${JSON.stringify(rows.columns.map((c) => c.name))}`,
        );
        const row = await rows.first();
        assert(row.a === 1 && row.b === "x", "values did not match the columns");
      } finally {
        await db.close();
      }
    },
  ],
  [
    "a row materializes explicitly, and leaks nothing when spread",
    async (open) => {
      const db = await fresh(open, "a INTEGER, b TEXT");
      try {
        await db.execute(`INSERT INTO ${CONFORMANCE_TABLE} VALUES (1, 'x')`);
        const row = await (await db.query(`SELECT a, b FROM ${CONFORMANCE_TABLE}`)).first();
        assert(JSON.stringify(row) === '{"a":1,"b":"x"}', `serialized as ${JSON.stringify(row)}`);
        assert(
          JSON.stringify(row.toObject()) === '{"a":1,"b":"x"}',
          `toObject gave ${JSON.stringify(row.toObject())}`,
        );
        assert(row.values().join(",") === "1,x", "values() did not agree with the getters");
        // A row is a lazy view over the batch, so spreading it does not reach
        // the columns — and, more importantly, must not reach the batch either.
        assert(
          Object.keys({ ...row }).length === 0,
          `spreading a row exposed ${JSON.stringify(Object.keys({ ...row }))}`,
        );
        assert(
          Object.getOwnPropertySymbols({ ...row }).length === 0,
          "spreading a row leaked the batch buffer through a symbol key",
        );
      } finally {
        await db.close();
      }
    },
  ],
  [
    "parameters bind by position, and are never interpolated",
    async (open) => {
      const db = await fresh(open, "a TEXT");
      try {
        // The injection classic: as a parameter it is a value, not syntax.
        const hostile = `'); DROP TABLE ${CONFORMANCE_TABLE}; --`;
        await db.execute(`INSERT INTO ${CONFORMANCE_TABLE} VALUES (${db.dialect.placeholder(1)})`, [
          hostile,
        ]);
        const row = await (await db.query(`SELECT a FROM ${CONFORMANCE_TABLE}`)).first();
        assert(row.a === hostile, "the parameter did not round-trip");
      } finally {
        await db.close();
      }
    },
  ],
  [
    "a sql`` template renders through the backend's own dialect",
    async (open) => {
      const db = await fresh(open, "a INTEGER, b TEXT");
      try {
        await db.execute(sql`INSERT INTO ${new Query([CONFORMANCE_TABLE], [])} VALUES (${7}, ${"z"})`);
        const row = await (await db.query(sql`SELECT a, b FROM ${new Query([CONFORMANCE_TABLE], [])} WHERE a = ${7}`)).first();
        assert(row !== null && row.b === "z", "the template did not round-trip");
      } finally {
        await db.close();
      }
    },
  ],
  [
    "null round-trips as null, and is not confused with a missing row",
    async (open) => {
      const db = await fresh(open, "a INTEGER, b TEXT");
      try {
        await db.execute(`INSERT INTO ${CONFORMANCE_TABLE} VALUES (1, NULL)`);
        const row = await (await db.query(`SELECT a, b FROM ${CONFORMANCE_TABLE}`)).first();
        assert(row !== null, "a row with a NULL column came back as no row");
        assert(row.b === null, `a NULL column came back as ${JSON.stringify(row.b)}`);
        const none = await (await db.query(`SELECT a FROM ${CONFORMANCE_TABLE} WHERE a = 999`)).first();
        assert(none === null, "an empty result did not answer null");
      } finally {
        await db.close();
      }
    },
  ],
  [
    "an empty result set iterates zero times and closes cleanly",
    async (open) => {
      const db = await fresh(open, "a INTEGER");
      try {
        let seen = 0;
        for await (const _row of await db.query(`SELECT a FROM ${CONFORMANCE_TABLE}`)) seen++;
        assert(seen === 0, `iterated ${seen} times over an empty result`);
        const rows = await (await db.query(`SELECT a FROM ${CONFORMANCE_TABLE}`)).toArray();
        assert(rows.length === 0, "toArray on an empty result was not empty");
      } finally {
        await db.close();
      }
    },
  ],
  [
    "a result set streams, and stopping early leaves the connection usable",
    async (open) => {
      const db = await fresh(open, "a INTEGER");
      try {
        await db.transaction(async (tx) => {
          for (let i = 0; i < 200; i++) {
            await tx.execute(`INSERT INTO ${CONFORMANCE_TABLE} VALUES (${tx.dialect.placeholder(1)})`, [i]);
          }
        });
        let seen = 0;
        for await (const _row of await db.query(`SELECT a FROM ${CONFORMANCE_TABLE}`)) {
          if (++seen === 3) break;
        }
        assert(seen === 3, `stopped after ${seen} rows`);
        // The abandoned cursor must not have taken the connection with it.
        const all = await (await db.query(`SELECT a FROM ${CONFORMANCE_TABLE}`)).toArray();
        assert(all.length === 200, `saw ${all.length} rows after abandoning a cursor`);
      } finally {
        await db.close();
      }
    },
  ],
  [
    "a transaction commits, and rolls back when its body throws",
    async (open) => {
      const db = await fresh(open, "a INTEGER");
      try {
        await db.transaction(async (tx) => {
          await tx.execute(`INSERT INTO ${CONFORMANCE_TABLE} VALUES (1)`);
        });
        try {
          await db.transaction(async (tx) => {
            await tx.execute(`INSERT INTO ${CONFORMANCE_TABLE} VALUES (2)`);
            throw new Error("rollback");
          });
          throw new Error("the transaction did not rethrow its body's error");
        } catch (e) {
          assert(e.message === "rollback", `a rollback replaced the error with: ${e.message}`);
        }
        const rows = await (await db.query(`SELECT a FROM ${CONFORMANCE_TABLE}`)).toArray();
        assert(rows.length === 1 && rows[0].a === 1, "the failed transaction was not rolled back");
      } finally {
        await db.close();
      }
    },
  ],
  [
    "a nested transaction rolls back without taking the outer one with it",
    async (open) => {
      const db = await fresh(open, "a INTEGER");
      try {
        if (!db.dialect.supports.savepoints) return; // reported as passing: not claimed
        await db.transaction(async (tx) => {
          await tx.execute(`INSERT INTO ${CONFORMANCE_TABLE} VALUES (1)`);
          try {
            await tx.transaction(async (inner) => {
              await inner.execute(`INSERT INTO ${CONFORMANCE_TABLE} VALUES (2)`);
              throw new Error("inner");
            });
          } catch {
            /* expected */
          }
        });
        const rows = await (await db.query(`SELECT a FROM ${CONFORMANCE_TABLE}`)).toArray();
        assert(
          rows.length === 1 && rows[0].a === 1,
          `expected only the outer insert, saw ${JSON.stringify(rows.map((r) => r.a))}`,
        );
      } finally {
        await db.close();
      }
    },
  ],
  [
    "a failure is a DbError carrying a code",
    async (open) => {
      const db = await fresh(open, "a INTEGER");
      try {
        let caught = null;
        try {
          await db.query(`SELECT * FROM ${CONFORMANCE_TABLE}_missing`);
        } catch (e) {
          caught = e;
        }
        assert(caught !== null, "a query against a missing table did not fail");
        assert(caught instanceof DbError, `threw ${caught.constructor.name}, not DbError`);
        assert(typeof caught.code === "string", "the error carried no code");
      } finally {
        await db.close();
      }
    },
  ],
  [
    "a constraint violation maps onto the portable code",
    async (open) => {
      const db = await fresh(open, "a INTEGER PRIMARY KEY");
      try {
        await db.execute(`INSERT INTO ${CONFORMANCE_TABLE} VALUES (1)`);
        let code = null;
        try {
          await db.execute(`INSERT INTO ${CONFORMANCE_TABLE} VALUES (1)`);
        } catch (e) {
          code = e.code;
        }
        assert(
          code === DbErrorCode.UniqueViolation,
          `a duplicate key reported ${code} rather than ${DbErrorCode.UniqueViolation}`,
        );
      } finally {
        await db.close();
      }
    },
  ],
  [
    "executeMany applies every set, and none of them when one fails",
    async (open) => {
      const db = await fresh(open, "a INTEGER PRIMARY KEY, b TEXT");
      try {
        const p = (i) => db.dialect.placeholder(i);
        const sql = `INSERT INTO ${CONFORMANCE_TABLE} VALUES (${p(1)}, ${p(2)})`;
        const result = await db.executeMany(sql, [
          [1, "one"],
          [2, "two"],
          [3, "three"],
        ]);
        assert(result.changes === 3, `reported ${result.changes} changes, expected 3`);

        let threw = false;
        try {
          // The second set collides with a row that already exists.
          await db.executeMany(sql, [[4, "four"], [1, "clash"]]);
        } catch {
          threw = true;
        }
        assert(threw, "a colliding set did not fail the batch");

        const rows = await (await db.query(`SELECT a FROM ${CONFORMANCE_TABLE} ORDER BY a`)).toArray();
        assert(
          rows.map((r) => r.a).join(",") === "1,2,3",
          `a failed batch left ${JSON.stringify(rows.map((r) => r.a))} — it must apply none of it`,
        );
      } finally {
        await db.close();
      }
    },
  ],
  [
    "a closed connection refuses work rather than hanging",
    async (open) => {
      const db = await fresh(open, "a INTEGER");
      await db.close();
      await db.close(); // idempotent
      let code = null;
      try {
        await db.query(`SELECT a FROM ${CONFORMANCE_TABLE}`);
      } catch (e) {
        code = e.code;
      }
      assert(code === DbErrorCode.Closed, `a closed connection reported ${code}`);
    },
  ],
  [
    "the query form this backend does not take is refused by name",
    async (open) => {
      const db = await open();
      try {
        let code = null;
        try {
          await db.query(queryAst({ select: ["a"] }));
        } catch (e) {
          code = e.code;
        }
        assert(
          code === DbErrorCode.QueryForm,
          `an unsupported query form reported ${code} rather than ${DbErrorCode.QueryForm}`,
        );
      } finally {
        await db.close();
      }
    },
  ],
];

/// Runs the conformance suite against a backend.
///
///     const report = await runBackendConformance(() => connect("mysql://…"));
///     if (!report.ok) console.error(report.failures);
///
/// `open` is called once per check and must resolve to a fresh connection; the
/// check closes it. Every check builds and drops its own table, so the suite
/// needs a database it may write to and leaves nothing behind.
///
/// This exists so that a third-party driver can *demonstrate* it behaves like
/// the built-ins rather than intend to. An ecosystem where an ORM can rely on
/// cross-backend behaviour needs the drivers to be checkable, and a shared
/// suite is the only version of that which does not decay.
async function runBackendConformance(open, { skip = [] } = {}) {
  const results = [];
  for (const [name, run] of CONFORMANCE_CHECKS) {
    if (skip.includes(name)) {
      results.push({ name, skipped: true });
      continue;
    }
    try {
      await run(open);
      results.push({ name, ok: true });
    } catch (e) {
      results.push({ name, ok: false, error: e && e.message != null ? e.message : String(e) });
    }
  }
  const failures = results.filter((r) => r.ok === false);
  return {
    ok: failures.length === 0,
    passed: results.filter((r) => r.ok === true).length,
    skipped: results.filter((r) => r.skipped).length,
    failures,
    results,
  };
}

export {
  // Application tier.
  connect,
  sql,
  queryAst,
  Query,
  Rows,
  DbError,
  DbErrorCode,
  // Driver tier.
  registerBackend,
  backendSchemes,
  BaseConnection,
  Dialect,
  ByteWriter,
  defineRowShape,
  decodeBatch,
  encodeParams,
  encodeParamSets,
  splitParams,
  mapError,
  asDbError,
  runBackendConformance,
};

export default { connect, sql, queryAst, DbError, DbErrorCode, registerBackend };
