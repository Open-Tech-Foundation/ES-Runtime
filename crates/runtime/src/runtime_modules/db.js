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

/// The accessor class for a backend whose values are **already JavaScript**.
///
/// The batch layout exists because a wire protocol hands over bytes, and
/// decoding them lazily is the whole performance argument of D56. A backend
/// that never had bytes — a document store answering JSON, a graph or vector
/// service over HTTP, an in-process engine holding objects — has nothing to
/// decode, and making it encode its values into the layout so that `decodeBatch`
/// can immediately take them apart again is pure loss. Two shapes, one `Row`
/// contract: the columns are prototype getters either way, so nothing
/// downstream can tell which kind it holds.
///
/// A record is an **array of values in column order**, which is what keeps one
/// accessor class monomorphic. `Rows.fromObjects` is the conversion for a
/// backend holding objects instead.
function defineRecordShape(columns) {
  class Row {
    // Private, for the same reason the byte shape's buffer is: a spread must
    // not carry the backing values out through a row.
    #values;

    constructor(values) {
      this.#values = values;
    }

    [READ](index) {
      const value = this.#values[index];
      return value === undefined ? null : value;
    }

    values() {
      return columns.map((_, i) => this[READ](i));
    }

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
  Row.decoders = [];
  Row.records = true;

  const seen = new Set();
  columns.forEach((column, index) => {
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
      // Which forms of query this backend takes. Defaulted so that every
      // backend written before these existed keeps its behaviour exactly: SQL
      // text, no AST.
      //
      // Both are declared rather than one being inferred from the other,
      // because a backend may take both — an engine that accepts an AST and
      // also parses SQL is a reasonable thing to be, and so is one that takes
      // neither form the other does.
      sqlText: true,
      queryAst: false,
      // Whether the backend has transactions at all. A backend that says no
      // gets a refusal from `transaction()` rather than a `BEGIN` it has never
      // heard of.
      transactions: true,
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

// Normalizes whatever a caller passed into `{ text, ast, positional, named }`.
//
// Exactly one of `text` and `ast` is non-null. Which forms a backend takes is
// its own declaration (`dialect.supports.sqlText` / `.queryAst`), so the door
// D56 opened for a backend that never speaks SQL is a capability check here
// rather than a refusal welded into this function. Both directions are
// expressible: an engine that wants an AST refuses text with the same code, and
// the same message shape, that a SQL engine refuses an AST with.
function normalizeQuery(q, params, dialect, backend) {
  if (isQueryAst(q)) {
    if (!dialect.supports.queryAst) {
      throw dbError(
        `the ${backend} backend takes SQL text, not a query AST`,
        DbErrorCode.QueryForm,
      );
    }
    const [positional, named] = splitParams(params);
    return { text: null, ast: q.ast, positional, named };
  }
  if (q instanceof Query) {
    if (!dialect.supports.sqlText) {
      throw dbError(
        `the ${backend} backend takes a query AST, not SQL text — build one with queryAst()`,
        DbErrorCode.QueryForm,
      );
    }
    const rendered = q.render(dialect);
    return { text: rendered.text, ast: null, positional: rendered.params, named: [] };
  }
  if (typeof q !== "string") {
    throw dbError(
      "a query must be SQL text, a sql`` template, or a query AST",
      DbErrorCode.QueryForm,
    );
  }
  if (!dialect.supports.sqlText) {
    throw dbError(
      `the ${backend} backend takes a query AST, not SQL text — build one with queryAst()`,
      DbErrorCode.QueryForm,
    );
  }
  const [positional, named] = splitParams(params);
  return { text: q, ast: null, positional, named };
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

  /// Rows already in hand, as a result set.
  ///
  /// For a backend whose answer is JavaScript rather than bytes: the records
  /// are wrapped, not encoded and decoded. `columns` may be omitted, in which
  /// case it is the union of the records' keys, in first-seen order — which is
  /// the right default for a document store, where the shape is the data's
  /// rather than the schema's.
  static fromObjects(records, columns) {
    const list = [...records];
    const shape = defineRecordShape(
      columns ?? inferColumns(list),
    );
    const names = shape.columns.map((column) => column.name);
    const rows = list.map((record) => names.map((name) => record[name]));
    let sent = false;
    return new Rows(
      {
        // The whole answer is here: there is no cursor to leave open, which a
        // pool reads as "this connection is free the moment the call returns".
        exhausted: true,
        async next() {
          if (sent) return { records: [], done: true };
          sent = true;
          return { records: rows, done: true };
        },
        async close() {},
      },
      shape,
    );
  }

  async *[Symbol.asyncIterator]() {
    try {
      while (!this._done) {
        const batch = await this._source.next(BATCH_BYTES);
        this._done = batch.done;
        // A batch arrives one of two ways, and which one is the backend's
        // nature rather than its choice: `bytes` for anything that read them
        // off a socket or out of an engine, `records` for a backend whose
        // values are already JavaScript. The branch is per batch, not per row.
        if (batch.records !== undefined) {
          for (const record of batch.records) yield new this._shape(record);
        } else if (batch.rows > 0) {
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

// The columns of a set of records: every key any of them has, in the order they
// were first seen. A document store's rows are not required to agree on a
// shape, and dropping the keys a later record introduced would lose data
// silently — the worst of the available failures.
function inferColumns(records) {
  const names = [];
  const seen = new Set();
  for (const record of records) {
    for (const name of Object.keys(record)) {
      if (seen.has(name)) continue;
      seen.add(name);
      names.push(name);
    }
  }
  return names.map((name) => ({ name, declType: null }));
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

  /**
   * Whether this connection is still worth using at all.
   *
   * A driver overrides it to account for a transport that died while nobody was
   * looking — a server restart, an idle timeout at the far end — which an
   * in-process engine cannot suffer and a socket can. A pool checks it before
   * handing a connection out, because otherwise the first anyone hears of a
   * dead connection is the next caller's error.
   */
  get usable() {
    return !this._closed;
  }

  /**
   * Whether this connection is fit for the **next** caller.
   *
   * The one question a protocol-blind pool cannot answer for itself, so it is
   * asked here, by one name, on every backend. The default is the part that is
   * true everywhere: alive, and not left inside a transaction. A driver adds
   * what its protocol knows — PostgreSQL's `ReadyForQuery` status, the Redis
   * database a stray `SELECT` moved the connection to — and anything not
   * vouched for is destroyed rather than reused, which is how an aborted
   * transaction or an open portal is stopped from leaking into the next
   * request.
   */
  get reusable() {
    return this.usable && this._depth === 0;
  }

  async query(q, params, options = {}) {
    this._open();
    const normalized = normalizeQuery(q, params, this.dialect, this.backend);
    const signal = options.signal;
    if (signal === undefined) return this._query(normalized);
    if (signal.aborted) throw signal.reason;

    const onAbort = () => {
      Promise.resolve(this._cancel()).catch(() => {});
    };
    signal.addEventListener("abort", onAbort, { once: true });
    let rows;
    try {
      rows = await this._query(normalized);
    } catch (e) {
      signal.removeEventListener("abort", onAbort);
      if (signal.aborted) throw signal.reason;
      throw e;
    }
    return this._bindSignalToRows(signal, rows, onAbort);
  }

  async execute(q, params, options = {}) {
    this._open();
    const normalized = normalizeQuery(q, params, this.dialect, this.backend);
    return this._withSignal(options.signal, () => this._execute(normalized));
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
    // The whole normalized query, not just its text: a backend taking an AST
    // has no text, and passing one field of a two-field union is how the AST
    // form ends up working everywhere except the batch path.
    const normalized = normalizeQuery(q, undefined, this.dialect, this.backend);
    const sets = rows.map((row) => splitParams(row));
    const run = () => this._executeMany(normalized, sets);
    // A backend without transactions cannot make a batch atomic, and wrapping
    // one in a transaction it does not have would fail every batch. It gets the
    // batch it can have, and the difference is stated in `supports`.
    return this._depth > 0 || !this.dialect.supports.transactions
      ? run()
      : this.transaction(run);
  }

  /**
   * Asks the backend to abandon whatever this connection is running.
   *
   * A driver overrides this with whatever its backend offers — a
   * `CancelRequest` on a second connection for a wire protocol, an interrupt
   * flag for an in-process engine. Not overriding it means `signal` still
   * *rejects the caller*, but the work keeps running until it finishes on its
   * own: the promise is abandoned, the statement is not. That is a meaningful
   * difference and the reason this is a method rather than an assumption.
   */
  async _cancel() {}

  /**
   * Runs `work` with an `AbortSignal` attached.
   *
   * Aborting asks the backend to cancel and then **waits** for it to answer.
   * Rejecting the caller the instant the signal fired would leave a statement
   * running and a connection mid-exchange; waiting a moment leaves both in a
   * known state, and the connection usable — which is the difference between
   * cancelling and hanging up.
   *
   * What the caller sees is their own `reason`, not whatever the backend calls
   * a cancelled statement. They asked; the backend's phrasing is a detail of
   * how the asking was carried out.
   */
  async _withSignal(signal, work) {
    if (signal === undefined) return work();
    if (signal.aborted) throw signal.reason;
    const onAbort = () => {
      Promise.resolve(this._cancel()).catch(() => {});
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
   * Keeps `signal` attached to a result set until its rows end.
   *
   * A streaming result is still the query running, and a caller who abandons
   * one halfway is exactly who wanted to cancel. The failure from an aborted
   * stream also arrives out of the *iterator* rather than out of the call that
   * started it, so the reason has to be translated in both places or `execute`
   * and `query` would report the same act differently.
   */
  _bindSignalToRows(signal, rows, onAbort) {
    if (rows.exhausted) {
      // Already complete: nothing left to cancel, nothing to keep listening for.
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
    const iterate = rows[Symbol.asyncIterator].bind(rows);
    rows[Symbol.asyncIterator] = async function* withSignal() {
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
        // must still run the inner one's cleanup, which closes the cursor.
        await inner.return?.(undefined);
      }
    };
    return rows;
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
  async _executeMany(query, sets) {
    let changes = 0;
    let lastInsertRowid = null;
    for (const [positional, named] of sets) {
      const result = await this._execute({ ...query, positional, named });
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
    if (!this.dialect.supports.transactions) {
      throw dbError(
        `the ${this.backend} backend has no transactions`,
        DbErrorCode.Unsupported,
      );
    }
    const depth = this._depth;
    const nested = depth > 0;
    if (nested && !this.dialect.supports.savepoints) {
      throw dbError(
        `the ${this.backend} backend has no savepoints, so transactions cannot nest`,
        DbErrorCode.Unsupported,
      );
    }
    const scope = { nested, name: nested ? `esrun_sp_${depth}` : null };
    await this._beginTransaction(scope);
    this._depth = depth + 1;
    try {
      const result = await fn(this);
      await this._commitTransaction(scope);
      return result;
    } catch (e) {
      // A rollback that itself fails must not replace the error that caused
      // it: the first failure is the one worth reporting, and the second is
      // usually a consequence of it.
      try {
        await this._rollbackTransaction(scope);
      } catch {
        /* the original error is the one that matters */
      }
      throw e;
    } finally {
      this._depth = depth;
    }
  }

  /**
   * The three statements a transaction is made of.
   *
   * They default to the SQL every SQL backend spells the same way, and they are
   * **methods** so that a backend which does not speak SQL can still have real
   * transactions — `MULTI`/`EXEC`, a protocol message, an engine call. Before
   * these existed the SQL was written into `transaction()` itself, which quietly
   * assumed that every backend was a SQL backend; a key-value store inheriting
   * that got a `BEGIN` sent to a server that has never heard of one.
   *
   * `scope` is `{ nested, name }`: `name` is the savepoint's, and is `null` at
   * the outermost level. A backend claiming `supports.savepoints` is the only
   * one that will ever see `nested: true`.
   */
  async _beginTransaction({ nested, name }) {
    await this.execute(nested ? `SAVEPOINT ${name}` : "BEGIN");
  }

  async _commitTransaction({ nested, name }) {
    await this.execute(nested ? `RELEASE ${name}` : "COMMIT");
  }

  async _rollbackTransaction({ nested, name }) {
    await this.execute(nested ? `ROLLBACK TO ${name}` : "ROLLBACK");
  }

  /**
   * Runs `fn` with a connection held for the whole of it.
   *
   * On a single connection that connection is `this`, and the method exists
   * anyway — because the alternative is that code which must not be spread over
   * two connections (a session setting, a `LISTEN`, a `WATCH`, a temporary
   * table) has to know whether it was handed a connection or a pool. An ORM
   * would then either demand a pool or duplicate itself. One name, both kinds,
   * and the pooled implementation is the one that actually borrows.
   */
  async withConnection(fn) {
    this._open();
    return fn(this);
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

  async _executeMany({ text }, sets) {
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

  /**
   * Interrupts whatever this connection is running.
   *
   * The engine runs its work on another thread, and this sets a flag that the
   * step loop checks — so it can land in the middle of a statement rather than
   * only between them, which is what makes cancellation here mean the same
   * thing it means to a networked backend.
   */
  async _cancel() {
    await ops.db_cancel(this._id);
  }

  async _close() {
    await ops.db_close(this._id);
  }
}

/// The database a `sqlite:` URL names.
///
/// Split out because the driver needs it twice: once to open, and once to
/// answer whether the URL can be pooled at all.
function sqlitePath(url) {
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
  return path;
}

async function connectSqlite(url, options) {
  const path = sqlitePath(url);
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
// Drivers
// ---------------------------------------------------------------------------

/// A backend, as a value.
///
/// A driver is imported and handed to `connect()` — `connect(url, { driver })` —
/// rather than installed into a registry by the side effect of importing it.
/// The difference is not stylistic. A registry makes the set of usable backends
/// depend on which modules happened to be evaluated, which is invisible at the
/// call site, order-dependent, and impossible to type: `connect()` could only
/// promise the portable `Connection`, so a driver's own surface needed a second
/// object to reach it. Passing the driver makes the backend part of the call,
/// so the connection that comes back is *that driver's* connection — Redis
/// commands and all — and nothing has to be registered, reserved, or replaced.
class Driver {
  constructor({ name, schemes, dialect, open, pooled }) {
    if (typeof name !== "string" || name === "") {
      throw dbError("a driver needs a name", DbErrorCode.Unsupported);
    }
    if (!Array.isArray(schemes) || schemes.length === 0) {
      throw dbError(
        `the ${name} driver must name the schemes it takes, e.g. schemes: ["${name}"]`,
        DbErrorCode.Unsupported,
      );
    }
    if (!(dialect instanceof Dialect)) {
      throw dbError(`the ${name} driver needs a Dialect`, DbErrorCode.Unsupported);
    }
    if (typeof open !== "function") {
      throw dbError(`the ${name} driver needs an open(url, options)`, DbErrorCode.Unsupported);
    }
    if (pooled !== undefined && typeof pooled !== "function") {
      throw dbError(`the ${name} driver's pooled must be a function`, DbErrorCode.Unsupported);
    }
    this.name = name;
    this.schemes = Object.freeze(schemes.map((scheme) => normalizeScheme(scheme, name)));
    this.dialect = dialect;
    this._open = open;
    this._pooled = pooled;
    Object.freeze(this);
  }

  /// Whether this driver takes URLs of that scheme, with or without the colon.
  accepts(scheme) {
    return this.schemes.includes(String(scheme).replace(/:$/, "").toLowerCase());
  }

  /// Opens one connection. `connect()` is the way callers reach this; a driver
  /// calls it directly when it opens connections of its own — a cluster client
  /// following a redirect, a pool filling a slot.
  open(url, options = {}) {
    return this._open(url, options);
  }

  /// Opens a pool that behaves like one connection.
  ///
  /// The default is `PooledConnection`, which is the whole of it for most
  /// backends. A driver overrides it to add its own surface to the pooled form
  /// — or to refuse pooling for a URL that cannot be pooled, which `sqlite:`
  /// does for `:memory:`.
  pooled(url, options = {}, poolOptions = {}) {
    return this._pooled === undefined
      ? new PooledConnection(this, url, options, poolOptions)
      : this._pooled(url, options, poolOptions);
  }
}

function normalizeScheme(scheme, driverName) {
  const name = String(scheme).replace(/:$/, "").toLowerCase();
  if (!/^[a-z][a-z0-9+.-]*$/.test(name)) {
    throw dbError(
      `${scheme} is not a usable scheme for the ${driverName} driver`,
      DbErrorCode.Unsupported,
    );
  }
  return name;
}

/// Defines a driver.
///
///     export default defineDriver({
///       name: "mydb",
///       schemes: ["mydb"],
///       dialect,
///       open: (url, options) => MyConnection.connect(url, options),
///     });
function defineDriver(spec) {
  return new Driver(spec ?? {});
}

/// The built-in SQLite driver.
///
/// It is an ordinary driver, defined with the same `defineDriver` a third party
/// uses and passed the same way. Being built in buys it nothing but the fact
/// that it is already imported: there is no privileged scheme, no seeded
/// registry entry, and nothing `connect()` knows about it that it does not know
/// about a driver published this morning.
const sqlite = defineDriver({
  name: "sqlite",
  schemes: ["sqlite"],
  dialect: SQLITE_DIALECT,
  open: connectSqlite,
  pooled(url, options, poolOptions) {
    // Every `sqlite::memory:` open is its **own** database — nothing is shared
    // between connections — so a pool of them is a pool of unrelated databases
    // handed out at random. Refused by name rather than silently.
    if (sqlitePath(url) === ":memory:") {
      throw dbError(
        "sqlite::memory: cannot be pooled: each connection would be its own database. Open it once, or use a file.",
        DbErrorCode.Unsupported,
      );
    }
    return new PooledConnection(sqlite, url, options, poolOptions);
  },
});

// ---------------------------------------------------------------------------
// Connecting
// ---------------------------------------------------------------------------

/// Opens a connection with the driver the caller passed.
///
///     import { connect, sqlite } from "runtime:db";
///     const db = await connect("sqlite:./app.db", { driver: sqlite });
///
///     import postgres from "@opentf/esrun-postgres";
///     const pg = await connect("postgres://user@host/app", { driver: postgres });
///
/// `pool: true` — or `pool: { max: 20 }` — gives a pool that presents the same
/// surface one connection does, so pooling is a property of the call rather
/// than a different object reached a different way.
///
/// The URL's scheme still matters: it is checked against the driver's, so
/// pointing the SQLite driver at a `postgres://` URL is caught here rather than
/// as a parse failure somewhere inside a driver that was never meant to see it.
async function connect(url, options = {}) {
  if (typeof url !== "string") {
    throw dbError("a connection string is required", DbErrorCode.Unsupported);
  }
  const { driver, pool, ...rest } = options ?? {};
  if (!(driver instanceof Driver)) {
    throw dbError(
      driver === undefined
        ? 'a driver is required: connect(url, { driver }). The built-in is `import { sqlite } from "runtime:db"`; others are packages, e.g. `import postgres from "@opentf/esrun-postgres"`.'
        : "options.driver must be a driver from defineDriver()",
      DbErrorCode.Unsupported,
    );
  }
  const colon = url.indexOf(":");
  if (colon <= 0) {
    throw dbError(
      `${url} is not a connection string: it needs a scheme, like sqlite:./app.db`,
      DbErrorCode.Unsupported,
    );
  }
  const scheme = url.slice(0, colon).toLowerCase();
  if (!driver.accepts(scheme)) {
    throw dbError(
      `the ${driver.name} driver does not take ${scheme}: URLs — it takes ${driver.schemes
        .map((s) => `${s}:`)
        .join(", ")}`,
      DbErrorCode.Unsupported,
    );
  }
  if (pool === undefined || pool === false) return driver.open(url, rest);
  return driver.pooled(url, rest, pool === true ? {} : pool);
}


// ---------------------------------------------------------------------------
// Pooling
// ---------------------------------------------------------------------------

/// A pool of connections, or of anything else a driver has to make and keep.
///
/// Protocol-blind on purpose: it knows how to make a thing, how to destroy one,
/// and how many to allow. What it cannot know is whether a returned connection
/// is **fit to reuse** — that needs the protocol, so the driver says so on
/// release, and anything not explicitly clean is destroyed rather than handed
/// to the next caller. Getting that backwards is how an aborted transaction or
/// an open portal leaks from one request into the next, which is a correctness
/// bug wearing a performance bug's clothes.
///
///     const pool = new Pool({
///       create: () => openConnection(url),
///       destroy: (c) => c.close(),
///       max: 10,
///     });
///     const c = await pool.acquire();
///     try { … } finally { pool.release(c, { clean: c.status === "idle" }); }
///
/// **Idle resources are swept on use, not on a timer.** A repeating timer would
/// keep the event loop alive for as long as the pool existed, so a program that
/// had finished its work would not exit — a poor trade for reaping a socket a
/// few seconds earlier. Call `close()` when done.
class Pool {
  constructor({
    create,
    destroy,
    validate,
    max = 10,
    idleTimeout = 30_000,
    acquireTimeout = 10_000,
  } = {}) {
    if (typeof create !== "function" || typeof destroy !== "function") {
      throw dbError("a pool needs create() and destroy()", DbErrorCode.Unsupported);
    }
    this._create = create;
    this._destroy = destroy;
    this._validate = validate;
    this._max = Math.max(1, max);
    this._idleTimeout = idleTimeout;
    this._acquireTimeout = acquireTimeout;
    // Idle entries, oldest first: `{ resource, since }`.
    this._idle = [];
    // Everything handed out and not yet returned.
    this._borrowed = new Set();
    this._waiting = [];
    this._closed = false;
  }

  /** How many resources exist, borrowed and idle together. */
  get size() {
    return this._idle.length + this._borrowed.size;
  }

  get idle() {
    return this._idle.length;
  }

  /** Callers queued behind a full pool. */
  get pending() {
    return this._waiting.length;
  }

  /** Destroys idle resources that have sat longer than the idle timeout. */
  _sweep() {
    if (this._idleTimeout <= 0) return;
    const cutoff = Date.now() - this._idleTimeout;
    while (this._idle.length > 0 && this._idle[0].since <= cutoff) {
      this._discard(this._idle.shift().resource);
    }
  }

  _discard(resource) {
    // A destroy that throws must not take the pool with it: the resource is
    // gone either way, and the caller asked for a release rather than a report.
    try {
      const result = this._destroy(resource);
      if (result && typeof result.catch === "function") result.catch(() => {});
    } catch {
      /* already being thrown away */
    }
  }

  async acquire() {
    if (this._closed) throw dbError("the pool is closed", DbErrorCode.Closed);
    this._sweep();

    for (;;) {
      const entry = this._idle.pop();
      if (entry === undefined) break;
      if (this._validate === undefined || (await this._validate(entry.resource))) {
        this._borrowed.add(entry.resource);
        return entry.resource;
      }
      this._discard(entry.resource);
    }

    if (this.size < this._max) {
      // The slot is taken before the resource exists, so a burst of callers
      // cannot each look, each see room, and together overshoot the maximum.
      const slot = Symbol("opening");
      this._borrowed.add(slot);
      try {
        const resource = await this._create();
        this._borrowed.delete(slot);
        this._borrowed.add(resource);
        return resource;
      } catch (e) {
        this._borrowed.delete(slot);
        // A failed create still has to wake someone, or a pool whose
        // connections all fail leaves every waiter parked on a slot that was
        // freed and never offered.
        this._wake();
        throw e;
      }
    }

    return this._wait();
  }

  _wait() {
    return new Promise((resolve, reject) => {
      const waiter = { resolve, reject, timer: undefined };
      if (this._acquireTimeout > 0) {
        waiter.timer = setTimeout(() => {
          const at = this._waiting.indexOf(waiter);
          if (at !== -1) this._waiting.splice(at, 1);
          reject(
            dbError(
              `waited ${this._acquireTimeout}ms for a connection and the pool stayed full (max ${this._max})`,
              DbErrorCode.Timeout,
            ),
          );
        }, this._acquireTimeout);
      }
      this._waiting.push(waiter);
    });
  }

  /** Hands the next waiter a slot, if anyone is queued. */
  _wake() {
    const waiter = this._waiting.shift();
    if (waiter === undefined) return false;
    clearTimeout(waiter.timer);
    // Through acquire() rather than straight at the resource: a waiter should
    // take the same route a fresh caller would, validation included.
    this.acquire().then(waiter.resolve, waiter.reject);
    return true;
  }

  /**
   * Returns a resource.
   *
   * `clean` is the driver's assertion that this resource is fit for the next
   * caller. It defaults to **false**, because the safe answer when nobody
   * checked is to throw the resource away.
   */
  release(resource, { clean = false } = {}) {
    if (!this._borrowed.delete(resource)) return;
    if (!clean || this._closed) {
      this._discard(resource);
      this._wake();
      return;
    }
    this._idle.push({ resource, since: Date.now() });
    this._wake();
  }

  /** Destroys every idle resource and refuses everyone still waiting. */
  async close() {
    this._closed = true;
    for (const waiter of this._waiting.splice(0)) {
      clearTimeout(waiter.timer);
      waiter.reject(dbError("the pool was closed", DbErrorCode.Closed));
    }
    for (const { resource } of this._idle.splice(0)) this._discard(resource);
    // Borrowed resources are left alone: their holders are still using them,
    // and will find the pool closed when they release.
  }
}

/// A pool that behaves like one connection.
///
/// The whole of `connect(url, { driver, pool: true })`. It implements the same
/// `Connection` surface a single connection does and borrows a real one per
/// call, so pooling is something a connection *is* rather than a different
/// object with a different shape reached through a different function. Every
/// driver got this wrong in its own way before it lived here: each had written
/// out the same acquire/release wrapper per method, and each had invented its
/// own answer to what a pooled connection's `query` returns.
///
/// A driver subclasses it to add its own surface — `withConnection` is here for
/// the things that are stateful across calls and therefore need one connection
/// held for the whole of them.
class PooledConnection {
  constructor(driver, url, options = {}, poolOptions = {}) {
    this.dialect = driver.dialect;
    this.backend = driver.name;
    this._driver = driver;
    this._pool = new Pool({
      create: () => driver.open(url, options),
      destroy: (connection) => connection.close(),
      // Checked on the way out as well as asserted on the way in: a connection
      // can die while nobody is holding it, and a pool that only asks on
      // release hands out the corpse.
      validate: (connection) => connection.usable !== false,
      max: poolOptions.max,
      idleTimeout: poolOptions.idleTimeout,
      acquireTimeout: poolOptions.acquireTimeout,
    });
  }

  /** Borrowed and idle together. */
  get size() {
    return this._pool.size;
  }

  get idle() {
    return this._pool.idle;
  }

  /** Callers queued behind a full pool. */
  get pending() {
    return this._pool.pending;
  }

  /**
   * The same two questions a single connection answers.
   *
   * A pool is usable until it is closed, and always fit for the next caller: a
   * connection that came back unfit was destroyed rather than kept, so the pool
   * itself never carries one caller's leftovers into the next. They are here so
   * that code holding "a connection" can ask without knowing which kind it has.
   */
  get usable() {
    return !this._pool._closed;
  }

  get reusable() {
    return this.usable;
  }

  /// Returns a connection, asking it whether it is fit for the next caller.
  /// `reusable` is strictly `true` or the connection is destroyed: a driver
  /// that never answered has not vouched for anything.
  _release(connection) {
    this._pool.release(connection, { clean: connection.reusable === true });
  }

  async query(q, params, options) {
    const connection = await this._pool.acquire();
    try {
      const rows = await connection.query(q, params, options);
      if (rows.exhausted) {
        // The whole result already arrived, so the connection is free before
        // the caller reads a row — which is most queries, and the case where
        // holding it until an iterator happened to finish would waste it.
        this._release(connection);
        return rows;
      }
      // A streaming result owns the connection until it ends. `Rows` closes
      // itself however the iteration finishes, so this rides on that rather
      // than asking the caller to remember.
      const close = rows.close.bind(rows);
      rows.close = async () => {
        try {
          await close();
        } finally {
          this._release(connection);
        }
      };
      return rows;
    } catch (e) {
      this._release(connection);
      throw e;
    }
  }

  async execute(q, params, options) {
    return this.withConnection((connection) => connection.execute(q, params, options));
  }

  async executeMany(q, rows) {
    return this.withConnection((connection) => connection.executeMany(q, rows));
  }

  /// Runs `fn` in a transaction on **one** connection, which is the point: a
  /// transaction spread across connections is not a transaction.
  async transaction(fn) {
    return this.withConnection((connection) => connection.transaction(fn));
  }

  /// Runs `fn` with one connection held for the whole of it.
  ///
  /// The escape hatch for everything that is stateful across calls — a session
  /// setting, a `LISTEN`, a `WATCH` — where borrowing per call would spread the
  /// state over connections that do not share it.
  async withConnection(fn) {
    const connection = await this._pool.acquire();
    try {
      return await fn(connection);
    } finally {
      this._release(connection);
    }
  }

  async close() {
    await this._pool.close();
  }

  async [Symbol.asyncDispose]() {
    await this.close();
  }
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

// A query in whichever form the backend takes.
//
// Only for the checks that are about the *connection* rather than about SQL.
// What it contains does not matter and it never reaches a backend: the check
// using it expects a refusal that happens before dispatch.
function anyForm(dialect) {
  return dialect.supports.sqlText ? `SELECT 1` : queryAst(null);
}

// Each check is `[name, async (open) => void, needs]` and throws to fail.
// A list rather than a framework: a driver author runs this from whatever they
// already use, and importing a test runner into the runtime to check a driver
// would be the wrong dependency in the wrong direction.
//
// `needs` is `"sql"` (the default) for a check written in SQL DDL and DML, and
// `"any"` for one that holds whatever form a backend takes. The distinction
// exists because most of this suite tests the *contract* through SQL, and a
// backend that never speaks SQL — which D56 has admitted as first-class since
// the first release — could otherwise only fail it. Failing a check you cannot
// express is not a finding, so those are skipped with a reason instead. What
// stays is the part that is genuinely about every backend.
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
    "an abort rejects with the caller's reason and leaves the connection usable",
    async (open) => {
      const db = await fresh(open, "a INTEGER");
      try {
        await db.execute(`INSERT INTO ${CONFORMANCE_TABLE} VALUES (1)`);

        // A signal already aborted never reaches the backend.
        const already = AbortSignal.abort(new Error("too late"));
        let early = "ran anyway";
        try {
          await db.query(`SELECT a FROM ${CONFORMANCE_TABLE}`, [], { signal: already });
        } catch (e) {
          early = e.message;
        }
        assert(early === "too late", `a pre-aborted signal gave ${early}`);

        // A signal that never fires changes nothing.
        const quiet = new AbortController();
        const rows = await (
          await db.query(`SELECT a FROM ${CONFORMANCE_TABLE}`, [], { signal: quiet.signal })
        ).toArray();
        assert(rows.length === 1, "an unaborted signal changed the result");

        // And the connection is unharmed by either.
        const after = await (await db.query(`SELECT a FROM ${CONFORMANCE_TABLE}`)).first();
        assert(after !== null, "the connection did not survive a signal");
      } finally {
        await db.close();
      }
    },
  ],
  [
    "a closed connection refuses work rather than hanging",
    async (open) => {
      const db = await open();
      await db.close();
      await db.close(); // idempotent
      let code = null;
      try {
        await db.query(anyForm(db.dialect));
      } catch (e) {
        code = e.code;
      }
      assert(code === DbErrorCode.Closed, `a closed connection reported ${code}`);
    },
    "any",
  ],
  [
    "the query form this backend does not take is refused by name",
    async (open) => {
      const db = await open();
      try {
        const { sqlText, queryAst: takesAst } = db.dialect.supports;
        // A backend that takes both forms has nothing to refuse, and demanding
        // that it refuse *something* would be inventing a requirement.
        if (sqlText && takesAst) return;
        const wrong = sqlText ? queryAst({ select: ["a"] }) : `SELECT 1`;
        let code = null;
        try {
          await db.query(wrong);
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
    "any",
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
  // One connection up front, purely to ask what this backend can express. A
  // check written in SQL says nothing about a backend that has no SQL, so it is
  // skipped **with a reason** rather than failed — and the reason is reported,
  // because "13 skipped" with no explanation is how a driver author concludes
  // they passed something they never ran.
  let speaksSql = true;
  try {
    const probe = await open();
    try {
      speaksSql = probe.dialect.supports.sqlText === true;
    } finally {
      await probe.close();
    }
  } catch (e) {
    return {
      ok: false,
      passed: 0,
      skipped: 0,
      failures: [{ name: "open a connection", ok: false, error: e?.message ?? String(e) }],
      results: [{ name: "open a connection", ok: false, error: e?.message ?? String(e) }],
    };
  }

  const results = [];
  for (const [name, run, needs = "sql"] of CONFORMANCE_CHECKS) {
    if (skip.includes(name)) {
      results.push({ name, skipped: true, reason: "skipped by the caller" });
      continue;
    }
    if (needs === "sql" && !speaksSql) {
      results.push({
        name,
        skipped: true,
        reason: "this check is written in SQL, and the backend takes a query AST",
      });
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
  sqlite,
  // Driver tier.
  defineDriver,
  Driver,
  BaseConnection,
  PooledConnection,
  Pool,
  Dialect,
  ByteWriter,
  defineRowShape,
  defineRecordShape,
  decodeBatch,
  encodeParams,
  encodeParamSets,
  splitParams,
  mapError,
  asDbError,
  runBackendConformance,
};

export default { connect, sql, queryAst, sqlite, defineDriver, DbError, DbErrorCode };
