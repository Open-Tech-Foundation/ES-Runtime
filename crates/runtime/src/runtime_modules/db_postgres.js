
// ---------------------------------------------------------------------------
// The built-in PostgreSQL driver (DECISIONS.md D147)
// ---------------------------------------------------------------------------
//
// Appended to db.js at build time, so it shares the module's scope. The
// protocol is in Rust (`pg_*` ops, crates/runtime/src/postgres): the
// handshake, the statement cache, the extended query protocol, and gathering
// `DataRow`s into batches. What is here is about values rather than bytes on
// a wire — the connection string and the `PG*` environment, parameters as
// text, columns as JavaScript — and the per-connection bookkeeping a pool
// asks about.

const PG_ENCODER = new TextEncoder();
const PG_DECODER = new TextDecoder();
const PG_MAX_SAFE = BigInt(Number.MAX_SAFE_INTEGER);
const PG_MIN_SAFE = -PG_MAX_SAFE;

/// Type OIDs decoded specially. Everything else stays a string.
const OID = Object.freeze({
  bool: 16,
  bytea: 17,
  int8: 20,
  int2: 21,
  int4: 23,
  text: 25,
  json: 114,
  float4: 700,
  float8: 701,
  varchar: 1043,
  date: 1082,
  time: 1083,
  timestamp: 1114,
  timestamptz: 1184,
  numeric: 1700,
  uuid: 2950,
  jsonb: 3802,
  interval: 1186,
});

function pgText(bytes, _view, start, length) {
  return PG_DECODER.decode(bytes.subarray(start, start + length));
}

/// Decoders written against text rather than byte spans, because array
/// elements arrive as substrings of the array literal.
const PG_FROM_TEXT = {
  [OID.bool]: (t) => t === "t",
  [OID.int2]: Number,
  [OID.int4]: Number,
  // A bigint only where a number would lose the value.
  [OID.int8]: (t) => {
    const value = BigInt(t);
    return value >= PG_MIN_SAFE && value <= PG_MAX_SAFE ? Number(value) : value;
  },
  [OID.float4]: Number,
  [OID.float8]: Number,
  // Arbitrary precision by definition: a double is the one representation
  // guaranteed to lose it.
  [OID.numeric]: (t) => t,
  [OID.json]: JSON.parse,
  [OID.jsonb]: JSON.parse,
  [OID.timestamptz]: (t) => new Date(t),
  [OID.timestamp]: (t) => new Date(`${t}Z`),
  [OID.date]: (t) => t,
  [OID.time]: (t) => t,
  [OID.bytea]: (t) => {
    if (!t.startsWith("\\x")) return PG_ENCODER.encode(t);
    const out = new Uint8Array((t.length - 2) / 2);
    for (let i = 0; i < out.length; i++) out[i] = Number.parseInt(t.substr(2 + i * 2, 2), 16);
    return out;
  },
};

/// Array type OIDs, mapped to their element types. The wire says nothing about
/// the relationship, so the pairs are listed; an array of any other type comes
/// back as its literal string rather than a guess.
const PG_ARRAY_ELEMENT = {
  199: OID.json,
  1000: OID.bool,
  1001: OID.bytea,
  1005: OID.int2,
  1007: OID.int4,
  1009: OID.text,
  1015: OID.varchar,
  1016: OID.int8,
  1021: OID.float4,
  1022: OID.float8,
  1115: OID.timestamp,
  1182: OID.date,
  1185: OID.timestamptz,
  1231: OID.numeric,
  2951: OID.uuid,
  3807: OID.jsonb,
};

/// PostgreSQL's array literal: `{1,2,3}`, `{"a,b",NULL}`, `{{1,2},{3,4}}`. An
/// unquoted `NULL` is the null element; a quoted `"NULL"` is the string.
function parsePgArray(literal, element) {
  let at = 0;
  const equals = literal.indexOf("=");
  if (literal.startsWith("[") && equals !== -1) at = equals + 1;

  function parseList() {
    at++;
    const out = [];
    if (literal[at] === "}") {
      at++;
      return out;
    }
    for (;;) {
      out.push(parseItem());
      if (literal[at] === ",") {
        at++;
        continue;
      }
      at++;
      return out;
    }
  }

  function parseItem() {
    if (literal[at] === "{") return parseList();
    if (literal[at] === '"') {
      at++;
      let text = "";
      while (at < literal.length && literal[at] !== '"') {
        text += literal[at] === "\\" ? literal[++at] : literal[at];
        at++;
      }
      at++;
      return element(text);
    }
    const start = at;
    while (at < literal.length && literal[at] !== "," && literal[at] !== "}") at++;
    const raw = literal.slice(start, at);
    return raw === "NULL" ? null : element(raw);
  }

  return literal[at] === "{" ? parseList() : [];
}

/// PostgreSQL counts time from 2000-01-01, not 1970.
const PG_EPOCH_MS = 946_684_800_000;
const PG_EPOCH_US = 946_684_800_000_000n;
const PG_EPOCH_DATE = "2000-01-01";

function instantFromMicros(micros) {
  return Temporal.Instant.fromEpochNanoseconds((micros + PG_EPOCH_US) * 1000n);
}

/// Binary decoders, only where binary is simpler and cheaper than text.
const PG_BINARY = {
  [OID.bool]: (bytes, _v, start) => bytes[start] !== 0,
  [OID.int2]: (_b, view, start) => view.getInt16(start),
  [OID.int4]: (_b, view, start) => view.getInt32(start),
  [OID.int8]: (_b, view, start) => {
    const value = view.getBigInt64(start);
    return value >= PG_MIN_SAFE && value <= PG_MAX_SAFE ? Number(value) : value;
  },
  [OID.float4]: (_b, view, start) => view.getFloat32(start),
  [OID.float8]: (_b, view, start) => view.getFloat64(start),
  [OID.bytea]: (bytes, _v, start, length) => bytes.slice(start, start + length),
  [OID.uuid]: (bytes, _v, start) => {
    let hex = "";
    for (let i = 0; i < 16; i++) hex += bytes[start + i].toString(16).padStart(2, "0");
    return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
  },
  [OID.timestamptz]: (_b, view, start) =>
    new Date(Number(view.getBigInt64(start) / 1000n) + PG_EPOCH_MS),
  [OID.timestamp]: (_b, view, start) =>
    new Date(Number(view.getBigInt64(start) / 1000n) + PG_EPOCH_MS),
  // A calendar day, not an instant: the same `YYYY-MM-DD` the text format sends.
  [OID.date]: (_b, view, start) =>
    new Date(view.getInt32(start) * 86_400_000 + PG_EPOCH_MS).toISOString().slice(0, 10),
};

/// Text decoders that produce Temporal values. PostgreSQL writes a space where
/// ISO wants a `T`, and `+00` where it wants `+00:00`.
const PG_FROM_TEXT_TEMPORAL = {
  [OID.date]: (t) => Temporal.PlainDate.from(t),
  [OID.time]: (t) => Temporal.PlainTime.from(t),
  [OID.timestamp]: (t) => Temporal.PlainDateTime.from(t.replace(" ", "T")),
  [OID.timestamptz]: (t) => Temporal.Instant.from(t.replace(" ", "T").replace(/([+-]\d{2})$/, "$1:00")),
  // ISO-8601, because the connection asks the server for that style.
  [OID.interval]: (t) => Temporal.Duration.from(t),
};

/// Binary decoders that produce Temporal values.
const PG_BINARY_TEMPORAL = {
  [OID.date]: (_b, view, start) =>
    Temporal.PlainDate.from(PG_EPOCH_DATE).add({ days: view.getInt32(start) }),
  [OID.time]: (_b, view, start) =>
    Temporal.PlainTime.from("00:00:00").add({ microseconds: Number(view.getBigInt64(start)) }),
  [OID.timestamp]: (_b, view, start) =>
    instantFromMicros(view.getBigInt64(start)).toZonedDateTimeISO("UTC").toPlainDateTime(),
  [OID.timestamptz]: (_b, view, start) => instantFromMicros(view.getBigInt64(start)),
  // Months, days and microseconds stay separate: a month is not thirty days.
  // Split into hours/minutes/seconds so it prints as the text path does.
  [OID.interval]: (_b, view, start) => {
    const total = view.getBigInt64(start);
    const days = view.getInt32(start + 8);
    const months = view.getInt32(start + 12);
    const hours = Number(total / 3_600_000_000n);
    const afterHours = total % 3_600_000_000n;
    const minutes = Number(afterHours / 60_000_000n);
    const afterMinutes = afterHours % 60_000_000n;
    const seconds = Number(afterMinutes / 1_000_000n);
    const microseconds = Number(afterMinutes % 1_000_000n);
    return Temporal.Duration.from({ months, days, hours, minutes, seconds, microseconds });
  },
};

/// The type OIDs a connection asks for in binary: the engine chooses each
/// column's format from this before it binds.
function pgBinaryOids(temporal) {
  const oids = new Set(Object.keys(PG_BINARY).map(Number));
  if (temporal) for (const oid of Object.keys(PG_BINARY_TEMPORAL)) oids.add(Number(oid));
  return [...oids];
}

function pgDecoderForFormat(oid, format, temporal) {
  if (format === 1) {
    const decode = (temporal ? PG_BINARY_TEMPORAL[oid] : undefined) ?? PG_BINARY[oid];
    if (decode !== undefined) return decode;
  }
  const elementOid = PG_ARRAY_ELEMENT[oid];
  if (elementOid !== undefined) {
    const element =
      (temporal ? PG_FROM_TEXT_TEMPORAL[elementOid] : undefined) ??
      PG_FROM_TEXT[elementOid] ??
      ((t) => t);
    return (b, v, s, l) => parsePgArray(pgText(b, v, s, l), element);
  }
  // `bool` is one byte and the answer is in it, so it skips the string.
  if (oid === OID.bool) return (bytes, _v, start) => bytes[start] === 0x74;
  const decode = (temporal ? PG_FROM_TEXT_TEMPORAL[oid] : undefined) ?? PG_FROM_TEXT[oid];
  return decode === undefined ? pgText : (b, v, s, l) => decode(pgText(b, v, s, l));
}

/// One parameter as its text, or `null` for SQL NULL.
function encodePgParam(value) {
  if (value === null || value === undefined) return null;
  if (typeof value === "string") return PG_ENCODER.encode(value);
  if (typeof value === "number") {
    if (!Number.isFinite(value)) {
      return PG_ENCODER.encode(Number.isNaN(value) ? "NaN" : value > 0 ? "Infinity" : "-Infinity");
    }
    return PG_ENCODER.encode(String(value));
  }
  if (typeof value === "bigint") return PG_ENCODER.encode(value.toString());
  if (typeof value === "boolean") return PG_ENCODER.encode(value ? "t" : "f");
  if (value instanceof Date) return PG_ENCODER.encode(value.toISOString());
  if (value instanceof Uint8Array) return PG_ENCODER.encode(pgHex(value));
  if (ArrayBuffer.isView(value)) {
    return PG_ENCODER.encode(pgHex(new Uint8Array(value.buffer, value.byteOffset, value.byteLength)));
  }
  if (value instanceof ArrayBuffer) return PG_ENCODER.encode(pgHex(new Uint8Array(value)));
  if (Array.isArray(value)) return PG_ENCODER.encode(pgArrayLiteral(value));
  if (typeof value === "object") return PG_ENCODER.encode(JSON.stringify(value));
  throw new TypeError(`a ${typeof value} cannot be bound as a query parameter`);
}

function pgHex(bytes) {
  let hex = "\\x";
  for (const byte of bytes) hex += byte.toString(16).padStart(2, "0");
  return hex;
}

function pgArrayLiteral(values) {
  const parts = values.map((value) => {
    if (value === null || value === undefined) return "NULL";
    if (Array.isArray(value)) return pgArrayLiteral(value);
    const text = value instanceof Date ? value.toISOString() : String(value);
    return `"${text.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;
  });
  return `{${parts.join(",")}}`;
}

const PG_NO_PARAMS = new Uint8Array(0);

/// The parameters as `Bind`'s parameter section — an `int16` count, then each
/// value's `int32` length (`-1` for NULL) and text — which the engine copies
/// into the message as-is.
function encodePgParams(positional) {
  if (positional.length === 0) return PG_NO_PARAMS;
  const encoded = positional.map(encodePgParam);
  let size = 2;
  for (const value of encoded) size += 4 + (value === null ? 0 : value.length);
  const w = new ByteWriter(size);
  w.i16(encoded.length);
  for (const value of encoded) {
    if (value === null) w.i32(-1);
    else w.i32(value.length).bytes(value);
  }
  return w.finish();
}

// -- errors -----------------------------------------------------------------

/// SQLSTATE → the portable code an application branches on. The five
/// characters are stable across versions and locales; the message is neither.
const PG_BY_SQLSTATE = {
  "23505": DbErrorCode.UniqueViolation,
  "23503": DbErrorCode.ForeignKeyViolation,
  "23502": DbErrorCode.NotNullViolation,
  "23514": DbErrorCode.CheckViolation,
  "40P01": DbErrorCode.Deadlock,
  "40001": DbErrorCode.SerializationFailure,
  "55P03": DbErrorCode.Busy,
  "57014": DbErrorCode.Timeout,
  "57P01": DbErrorCode.ConnectionLost,
  "57P02": DbErrorCode.ConnectionLost,
  "57P03": DbErrorCode.ConnectionLost,
  "28000": DbErrorCode.AuthFailed,
  "28P01": DbErrorCode.AuthFailed,
  "42601": DbErrorCode.Syntax,
  "42P01": DbErrorCode.UndefinedTable,
  "42703": DbErrorCode.UndefinedColumn,
  "25006": DbErrorCode.ReadOnly,
  "0A000": DbErrorCode.Unsupported,
  "53300": DbErrorCode.Throttled,
  "53400": DbErrorCode.Throttled,
};

/// The class of a SQLSTATE — its first two characters — for the codes with no
/// exact entry.
function pgPortableCode(sqlstate) {
  const exact = PG_BY_SQLSTATE[sqlstate];
  if (exact) return exact;
  switch (sqlstate.slice(0, 2)) {
    case "08":
      return DbErrorCode.ConnectionLost;
    case "28":
      return DbErrorCode.AuthFailed;
    case "40":
      return DbErrorCode.SerializationFailure;
    case "42":
      return DbErrorCode.Syntax;
    default:
      return DbErrorCode.Backend;
  }
}

/// A server message's fields, as the engine hands them over by their one-byte
/// codes, in the shape the driver has always exposed.
function pgServerMessage(fields) {
  const out = {
    severity: fields.S ?? fields.V ?? "ERROR",
    code: fields.C ?? "",
    message: fields.M ?? "the server reported an error with no message",
  };
  if (fields.D !== undefined) out.detail = fields.D;
  if (fields.H !== undefined) out.hint = fields.H;
  if (fields.P !== undefined) out.position = fields.P;
  if (fields.s !== undefined) out.schema = fields.s;
  if (fields.t !== undefined) out.table = fields.t;
  if (fields.c !== undefined) out.column = fields.c;
  if (fields.n !== undefined) out.constraint = fields.n;
  return out;
}

const PG_FAILURE_CODES = {
  lost: DbErrorCode.ConnectionLost,
  auth: DbErrorCode.AuthFailed,
  busy: DbErrorCode.ConnectionBusy,
  unsupported: DbErrorCode.Unsupported,
};

/// The `DbError` for an op's `{ error }`. A server error keeps everything the
/// server said: an ORM wants the constraint name, and a human wants the hint.
function pgError(failure) {
  if (failure.kind === "server") {
    const server = pgServerMessage(failure.fields);
    const error = asDbError(
      Object.assign(new Error(server.message), { code: server.code }),
      pgPortableCode(server.code),
    );
    return Object.assign(error, { server });
  }
  return new DbError(failure.message, {
    code: PG_FAILURE_CODES[failure.kind] ?? DbErrorCode.Backend,
  });
}

// -- the connection string ----------------------------------------------------

const POSTGRES_DIALECT = new Dialect({
  name: "postgres",
  placeholder: (index) => `$${index}`,
  supports: {
    returning: true,
    savepoints: true,
    // The wire protocol binds by position only.
    namedParameters: false,
    // LISTEN/NOTIFY is the next phase of D147; until it lands the driver says
    // so rather than accepting a subscribe it cannot deliver.
    subscriptions: false,
  },
});

/// The `PG*` variables every libpq tool reads: defaults below the URL and
/// explicit options. Reading them needs `Env`; without it there are no
/// defaults, which is not an error.
///
/// Imported when a connection opens rather than with this module: `runtime:db`
/// is loaded by programs that never touch PostgreSQL, and by embedders with no
/// process to describe, and neither should pay for — or fail on — this.
async function pgEnvironmentDefaults() {
  const options = {};
  try {
    const { env: processEnv, unmask: unmaskSecret } = await import("runtime:process");
    if (processEnv.PGHOST) options.host = String(processEnv.PGHOST);
    if (processEnv.PGPORT) options.port = Number(processEnv.PGPORT);
    if (processEnv.PGUSER) options.user = String(processEnv.PGUSER);
    if (processEnv.PGDATABASE) options.database = String(processEnv.PGDATABASE);
    if (processEnv.PGAPPNAME) options.applicationName = String(processEnv.PGAPPNAME);
    // `unmask` always: a plain string passes through unchanged, and a masked
    // one must not reach a startup packet as "[secret]".
    if (processEnv.PGPASSWORD) options.password = String(unmaskSecret(processEnv.PGPASSWORD));
    const sslmode = processEnv.PGSSLMODE ? String(processEnv.PGSSLMODE) : "";
    if (sslmode === "require" || sslmode === "prefer" || sslmode === "disable") {
      options.sslmode = sslmode;
    }
    if (processEnv.PGCONNECT_TIMEOUT) {
      const seconds = Number(processEnv.PGCONNECT_TIMEOUT);
      if (Number.isFinite(seconds) && seconds >= 0) options.connectTimeout = seconds * 1000;
    }
  } catch {
    return {};
  }
  return options;
}

/// `postgres://user:password@host:port/database?sslmode=require` → options.
/// Precedence, highest first: explicit options, the URL, `PG*` (`environment`),
/// defaults. libpq spells `connect_timeout` in seconds; the options object
/// means milliseconds, as the rest of JavaScript does.
function parsePgConnectionString(url, overrides = {}, environment = {}) {
  const parsed = new URL(url);
  const options = {};
  if (parsed.hostname !== "") options.host = decodeURIComponent(parsed.hostname);
  if (parsed.port !== "") options.port = Number(parsed.port);
  const database = decodeURIComponent(parsed.pathname.replace(/^\//, ""));
  if (database !== "") options.database = database;
  if (parsed.username !== "") options.user = decodeURIComponent(parsed.username);
  if (parsed.password !== "") options.password = decodeURIComponent(parsed.password);
  const connectSeconds = parsed.searchParams.get("connect_timeout");
  if (connectSeconds !== null && connectSeconds !== "") {
    const seconds = Number(connectSeconds);
    if (Number.isFinite(seconds) && seconds >= 0) options.connectTimeout = seconds * 1000;
  }
  const statementMs = parsed.searchParams.get("statement_timeout");
  if (statementMs !== null && statementMs !== "") {
    const ms = Number(statementMs);
    if (Number.isFinite(ms) && ms >= 0) options.statementTimeout = ms;
  }
  // libpq's `sslrootcert` names a file; this takes the certificate itself,
  // because reading a file is a capability a URL should not exercise.
  const rootCert = parsed.searchParams.get("sslrootcert");
  if (rootCert !== null && rootCert !== "") options.sslRootCert = rootCert;
  const sslmode = parsed.searchParams.get("sslmode");
  if (sslmode === "require" || sslmode === "prefer" || sslmode === "disable") {
    options.sslmode = sslmode;
  }
  const application = parsed.searchParams.get("application_name");
  if (application !== null) options.applicationName = application;
  return {
    host: "localhost",
    port: 5432,
    ...environment,
    ...pgDefined(options),
    ...pgDefined(overrides),
  };
}

function pgDefined(value) {
  return Object.fromEntries(Object.entries(value).filter(([, v]) => v !== undefined));
}

// -- rows ----------------------------------------------------------------------

/// Row classes, one per shape for the whole process: a caller's `row.id`
/// stays monomorphic only while the rows at that line share one class.
const PG_SHAPES = new Map();

function pgRowShape(names, oids, formats, temporal) {
  const key = `${temporal ? 1 : 0}|${formats.join(",")}|${oids.join(",")}|${names.join("\u0000")}`;
  let shape = PG_SHAPES.get(key);
  if (shape === undefined) {
    shape = defineRowShape(
      names.map((name, i) => ({ name, declType: null, oid: oids[i] })),
      { decoders: oids.map((oid, i) => pgDecoderForFormat(oid, formats[i] ?? 0, temporal)) },
    );
    // Bounded, oldest first: generated SQL can produce shapes without end.
    if (PG_SHAPES.size >= SHAPE_LIMIT) PG_SHAPES.delete(PG_SHAPES.keys().next().value);
  } else {
    PG_SHAPES.delete(key);
  }
  PG_SHAPES.set(key, shape);
  return shape;
}

const PG_NO_ROWS_SHAPE = defineRowShape([]);

/// `INSERT 0 3` / `UPDATE 2` / `SELECT 7`: the count is the last word.
function pgAffectedRows(tag) {
  const parts = tag.trim().split(" ");
  const count = Number(parts[parts.length - 1]);
  return Number.isFinite(count) ? count : 0;
}

/// Nothing: a statement with no result set.
const PG_EMPTY_SOURCE = {
  exhausted: true,
  async next() {
    return { bytes: new Uint8Array(0), rows: 0, done: true };
  },
  async close() {},
};

// -- the connection ------------------------------------------------------------

/// Names a connect in flight, so a timeout can abort it (`pg_abort_connect`).
let pgNextTicket = 1;

class PgConnection extends BaseConnection {
  constructor() {
    super({ dialect: POSTGRES_DIALECT, backend: "postgres" });
    this._id = null;
    this._fatal = null;
    /// `ReadyForQuery`'s transaction status: `I`, `T` or `E`.
    this.status = "I";
    /// The GUCs the server reports as they change: time zone, encoding, …
    this.parameters = {};
    /// Called for each NOTICE/WARNING. Unset, they are discarded — a driver
    /// that printed on its own would be one you had to work around.
    this.onNotice = undefined;
    this._temporal = true;
    /// Row classes by the engine's statement id, dropped as the engine evicts.
    this._shapes = new Map();
    this._streaming = false;
    this._locked = false;
    this._waiters = [];
    this._unlock = () => {
      const next = this._waiters.shift();
      if (next === undefined) this._locked = false;
      else next();
    };
    this._target = null;
    this._processId = 0;
  }

  /// Whether the connection can run another exchange.
  get usable() {
    return this._fatal === null && this._id !== null && !this._closed;
  }

  /// Whether it is fit for the next caller: PostgreSQL says so itself in every
  /// ReadyForQuery, and `T`/`E` would leak a transaction into the next borrower.
  get reusable() {
    return this.usable && this.status === "I";
  }

  async open(options) {
    const budget = options.connectTimeout ?? 10_000;
    const ticket = pgNextTicket++;
    const opening = this._connect(options, ticket);
    if (budget <= 0) return opening;
    let timer;
    const expired = new Promise((_, reject) => {
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
      await Promise.race([opening, expired]);
    } catch (e) {
      // The handshake is abandoned by closing its socket — a server that
      // accepted and went silent would hold it open for good — and a
      // connection that completes anyway is closed when it does.
      opening.then(() => this._close()).catch(() => {});
      await ops.pg_abort_connect(ticket);
      throw e;
    } finally {
      clearTimeout(timer);
    }
  }

  async _connect(options, ticket) {
    this._target = options;
    this._temporal = options.temporal !== false;
    const user = options.user ?? "postgres";
    const params = [
      "user",
      user,
      "database",
      options.database ?? user,
      "application_name",
      options.applicationName ?? "esrun",
      "client_encoding",
      "UTF8",
    ];
    // ISO-8601 intervals, so the text path reads what Temporal.Duration parses.
    if (this._temporal) params.push("IntervalStyle", "iso_8601");
    if (options.statementTimeout !== undefined && options.statementTimeout > 0) {
      // Enforced by the server, from the first statement.
      params.push("statement_timeout", String(Math.trunc(options.statementTimeout)));
    }
    const cacheLimit =
      options.preparedStatementCacheSize === undefined
        ? 100
        : Math.max(0, Math.trunc(options.preparedStatementCacheSize));
    const ca =
      options.sslRootCert === undefined
        ? PG_NO_PARAMS
        : typeof options.sslRootCert === "string"
          ? PG_ENCODER.encode(options.sslRootCert)
          : options.sslRootCert;
    const result = await ops.pg_connect(
      options.host ?? "localhost",
      options.port ?? 5432,
      options.sslmode ?? "prefer",
      ca,
      params,
      // SASLprep's part that matters in practice: NFKC. The rest of it is left
      // out, and an ASCII password is unaffected either way.
      options.password === undefined ? null : options.password.normalize("NFKC"),
      cacheLimit,
      pgBinaryOids(this._temporal),
      ticket,
    );
    if (result.error !== undefined) {
      this._aside(result);
      throw pgError(result.error);
    }
    this._id = result.id;
    this._processId = result.processId;
    this._aside(result);
  }

  /// Takes what the engine reported beside its answer.
  _aside(result) {
    if (result.status !== undefined) this.status = result.status;
    if (result.parameters !== undefined) {
      for (const [key, value] of result.parameters) this.parameters[key] = value;
    }
    if (result.notices !== undefined && this.onNotice !== undefined) {
      for (const notice of result.notices) this.onNotice(pgServerMessage(notice));
    }
    if (result.evicted !== undefined) {
      for (const id of result.evicted) this._shapes.delete(id);
    }
    if (result.error !== undefined) {
      const { kind } = result.error;
      if (kind === "lost") this._fatal = result.error;
      if (kind !== "busy") this._streaming = false;
    }
  }

  /// The connection for one exchange: at once when it is free, which is almost
  /// always, or after the exchanges ahead of it. A result set still being read
  /// is refused rather than queued, because only its reader can finish it.
  _acquire() {
    if (this._fatal !== null) throw pgError(this._fatal);
    if (this._id === null) {
      throw new DbError("the connection is closed", { code: DbErrorCode.Closed });
    }
    if (this._streaming) {
      throw new DbError(
        "this connection is streaming a result set — finish it (await rows.toArray(), or let the for-await end), or run the second query on another connection",
        { code: DbErrorCode.ConnectionBusy },
      );
    }
    if (!this._locked) {
      this._locked = true;
      return null;
    }
    return new Promise((resolve) => this._waiters.push(resolve));
  }

  _rejectNamed(named) {
    if (named.length > 0) {
      throw new DbError(
        "PostgreSQL binds parameters by position; pass an array and use $1, $2, … (or the sql`` tag)",
        { code: DbErrorCode.Unsupported },
      );
    }
  }

  async _query({ text, positional, named }) {
    this._rejectNamed(named);
    const waiting = this._acquire();
    if (waiting !== null) await waiting;
    let held = true;
    const release = () => {
      if (!held) return;
      held = false;
      this._streaming = false;
      this._unlock();
    };
    let result;
    try {
      result = await ops.pg_query(this._id, text ?? "", encodePgParams(positional), BATCH_BYTES, false);
    } catch (e) {
      release();
      throw e;
    }
    this._aside(result);
    if (result.error !== undefined) {
      release();
      throw pgError(result.error);
    }
    let shape = this._shapes.get(result.statement);
    if (result.names !== undefined) {
      shape = result.hasRows
        ? pgRowShape(result.names, result.oids, result.formats, this._temporal)
        : null;
      this._shapes.set(result.statement, shape);
    }
    if (shape === null || shape === undefined) {
      release();
      return new Rows(PG_EMPTY_SOURCE, PG_NO_ROWS_SHAPE);
    }
    const first = { bytes: result.bytes, rows: result.rows, done: result.done };
    if (first.done) {
      // The whole result arrived with the query, so the connection is free
      // before the caller reads a single row.
      release();
      return new Rows(pgOneBatch(first), shape);
    }
    // More to come: the result set holds the connection until it ends.
    this._streaming = true;
    const self = this;
    let pending = first;
    return new Rows(
      {
        exhausted: false,
        async next(maxBytes) {
          if (pending !== null) {
            const batch = pending;
            pending = null;
            return batch;
          }
          const next = await ops.pg_fetch(self._id, maxBytes ?? BATCH_BYTES);
          self._aside(next);
          if (next.error !== undefined) {
            release();
            throw pgError(next.error);
          }
          if (next.done) release();
          return next;
        },
        async close() {
          if (!held) return;
          // A caller that stopped early left the server mid-result: the rest
          // comes off the wire before anything else can be asked.
          try {
            const finished = await ops.pg_finish(self._id);
            self._aside(finished);
            if (finished.error !== undefined) throw pgError(finished.error);
          } finally {
            release();
          }
        },
      },
      shape,
    );
  }

  async _execute({ text, positional, named }) {
    this._rejectNamed(named);
    const waiting = this._acquire();
    if (waiting !== null) await waiting;
    try {
      const result = await ops.pg_query(this._id, text ?? "", encodePgParams(positional), 0, true);
      this._aside(result);
      if (result.error !== undefined) throw pgError(result.error);
      return { changes: pgAffectedRows(result.tag), lastInsertRowid: null };
    } finally {
      this._unlock();
    }
  }

  async _close() {
    const id = this._id;
    if (id === null) return;
    this._id = null;
    await ops.pg_close(id);
  }
}

function pgOneBatch(batch) {
  let pending = batch;
  return {
    exhausted: true,
    async next() {
      const value = pending ?? { bytes: new Uint8Array(0), rows: 0, done: true };
      pending = null;
      return value;
    },
    async close() {},
  };
}

/// A pool of PostgreSQL connections, presenting the surface one connection does.
class PgPooled extends PooledConnection {}

/// The built-in PostgreSQL driver: `postgres:` and `postgresql:` URLs, both in
/// the wild and neither more correct. Explicit options win over the URL, and
/// the URL over `PG*`.
const postgres = defineDriver({
  name: "postgres",
  schemes: ["postgres", "postgresql"],
  dialect: POSTGRES_DIALECT,
  async open(url, options = {}) {
    const connection = new PgConnection();
    await connection.open(parsePgConnectionString(url, options, await pgEnvironmentDefaults()));
    return connection;
  },
  /// Nothing is opened here: connections are made when first needed, each
  /// reading `PG*` as it opens. Parsed once now all the same, so a malformed
  /// string fails where it was written.
  pooled(url, options = {}, poolOptions = {}) {
    parsePgConnectionString(url, options);
    return new PgPooled(postgres, url, options, poolOptions);
  },
});

export { postgres };
