
// ---------------------------------------------------------------------------
// The built-in MySQL (and MariaDB) driver (DECISIONS.md D147)
// ---------------------------------------------------------------------------
//
// Appended to db.js at build time, after the PostgreSQL driver. The protocol
// is in Rust (`my_*` ops, crates/runtime/src/mysql): the handshake and its TLS
// upgrade, authentication, the statement cache, and transcoding rows into the
// shared layout. What is here is about values — the parameter block,
// decoding columns, the connection string and `MYSQL_*` — and the bookkeeping
// a pool asks about.

/// `enum_field_types`.
const MY = Object.freeze({
  DECIMAL: 0x00,
  TINY: 0x01,
  SHORT: 0x02,
  LONG: 0x03,
  FLOAT: 0x04,
  DOUBLE: 0x05,
  NULL: 0x06,
  TIMESTAMP: 0x07,
  LONGLONG: 0x08,
  INT24: 0x09,
  DATE: 0x0a,
  TIME: 0x0b,
  DATETIME: 0x0c,
  YEAR: 0x0d,
  VARCHAR: 0x0f,
  BIT: 0x10,
  VECTOR: 0xf2,
  JSON: 0xf5,
  NEWDECIMAL: 0xf6,
  ENUM: 0xf7,
  SET: 0xf8,
  TINY_BLOB: 0xf9,
  MEDIUM_BLOB: 0xfa,
  LONG_BLOB: 0xfb,
  BLOB: 0xfc,
  VAR_STRING: 0xfd,
  STRING: 0xfe,
  GEOMETRY: 0xff,
});

const MY_UNSIGNED_FLAG = 0x20;
/// The character set id MySQL uses for "no character set: these are bytes".
const MY_BINARY_CHARSET = 63;
/// Server status: a transaction is open.
const MY_STATUS_IN_TRANS = 0x1;

const MY_ENCODER = new TextEncoder();
const MY_DECODER = new TextDecoder();

const myText = (bytes, _view, start, length) => MY_DECODER.decode(bytes.subarray(start, start + length));
const myRaw = (bytes, _view, start, length) => bytes.slice(start, start + length);

const MY_DECODERS = {
  i8: (_b, view, start) => view.getInt8(start),
  u8: (_b, view, start) => view.getUint8(start),
  i16: (_b, view, start) => view.getInt16(start, true),
  u16: (_b, view, start) => view.getUint16(start, true),
  i32: (_b, view, start) => view.getInt32(start, true),
  u32: (_b, view, start) => view.getUint32(start, true),
  // A bigint only where a number would lose the value.
  i64: (_b, view, start) => {
    const high = view.getInt32(start + 4, true);
    if (high > -0x20_0000 && high < 0x20_0000) {
      return high * 0x1_0000_0000 + view.getUint32(start, true);
    }
    return view.getBigInt64(start, true);
  },
  u64: (_b, view, start) => {
    const high = view.getUint32(start + 4, true);
    if (high < 0x20_0000) return high * 0x1_0000_0000 + view.getUint32(start, true);
    return view.getBigUint64(start, true);
  },
  f32: (_b, view, start) => view.getFloat32(start, true),
  f64: (_b, view, start) => view.getFloat64(start, true),
  json: (bytes, view, start, length) => JSON.parse(myText(bytes, view, start, length)),
};

/// A date or time struct's fields: absent ones are zero, which is what MySQL means.
function myDateParts(view, start, length) {
  return {
    year: length >= 4 ? view.getUint16(start, true) : 0,
    month: length >= 4 ? view.getUint8(start + 2) : 0,
    day: length >= 4 ? view.getUint8(start + 3) : 0,
    hour: length >= 7 ? view.getUint8(start + 4) : 0,
    minute: length >= 7 ? view.getUint8(start + 5) : 0,
    second: length >= 7 ? view.getUint8(start + 6) : 0,
    micro: length >= 11 ? view.getUint32(start + 7, true) : 0,
  };
}

const myPad = (n, width = 2) => String(n).padStart(width, "0");

/// The text MySQL itself prints, for the zero date no calendar has.
function myDateTimeText(view, start, length, withTime) {
  const d = myDateParts(view, start, length);
  const date = `${myPad(d.year, 4)}-${myPad(d.month)}-${myPad(d.day)}`;
  if (!withTime) return date;
  const fraction = d.micro === 0 ? "" : `.${myPad(d.micro, 6)}`;
  return `${date} ${myPad(d.hour)}:${myPad(d.minute)}:${myPad(d.second)}${fraction}`;
}

function myUtcMillis(d) {
  const date = new Date(0);
  date.setUTCFullYear(d.year, d.month - 1, d.day);
  date.setUTCHours(d.hour, d.minute, d.second, Math.floor(d.micro / 1000));
  return date.getTime();
}

function myIsZeroDate(view, start, length) {
  return length === 0 || (view.getUint16(start, true) === 0 && view.getUint8(start + 2) === 0);
}

const MY_TEMPORAL = {
  date: (_b, view, start, length) => {
    if (myIsZeroDate(view, start, length)) return myDateTimeText(view, start, length, false);
    const d = myDateParts(view, start, length);
    return new Temporal.PlainDate(d.year, d.month, d.day);
  },
  datetime: (_b, view, start, length) => {
    if (myIsZeroDate(view, start, length)) return myDateTimeText(view, start, length, true);
    const d = myDateParts(view, start, length);
    const ms = Math.floor(d.micro / 1000);
    return new Temporal.PlainDateTime(d.year, d.month, d.day, d.hour, d.minute, d.second, ms, d.micro - ms * 1000);
  },
  // A TIMESTAMP is an instant; the session is UTC, so the wall time is UTC.
  timestamp: (_b, view, start, length) => {
    if (myIsZeroDate(view, start, length)) return myDateTimeText(view, start, length, true);
    const d = myDateParts(view, start, length);
    return Temporal.Instant.fromEpochNanoseconds(
      BigInt(myUtcMillis(d)) * 1_000_000n + BigInt(d.micro % 1000) * 1000n,
    );
  },
  // A TIME is a duration, not a time of day: -838:59:59 to 838:59:59.
  time: (_b, view, start, length) => {
    if (length === 0) return new Temporal.Duration();
    const sign = view.getUint8(start) === 1 ? -1 : 1;
    const days = view.getUint32(start + 1, true);
    const micro = length >= 12 ? view.getUint32(start + 8, true) : 0;
    const ms = Math.floor(micro / 1000);
    return new Temporal.Duration(
      0,
      0,
      0,
      0,
      sign * (days * 24 + view.getUint8(start + 5)),
      sign * view.getUint8(start + 6),
      sign * view.getUint8(start + 7),
      sign * ms,
      sign * (micro - ms * 1000),
    );
  },
};

const MY_LEGACY = {
  date: (_b, view, start, length) => myDateTimeText(view, start, length, false),
  datetime: (_b, view, start, length) =>
    myIsZeroDate(view, start, length)
      ? myDateTimeText(view, start, length, true)
      : new Date(myUtcMillis(myDateParts(view, start, length))),
  timestamp: (_b, view, start, length) =>
    myIsZeroDate(view, start, length)
      ? myDateTimeText(view, start, length, true)
      : new Date(myUtcMillis(myDateParts(view, start, length))),
  time: (_b, view, start, length) => {
    if (length === 0) return "00:00:00";
    const sign = view.getUint8(start) === 1 ? "-" : "";
    const hours = view.getUint32(start + 1, true) * 24 + view.getUint8(start + 5);
    const micro = length >= 12 ? view.getUint32(start + 8, true) : 0;
    const fraction = micro === 0 ? "" : `.${myPad(micro, 6)}`;
    return `${sign}${myPad(hours)}:${myPad(view.getUint8(start + 6))}:${myPad(view.getUint8(start + 7))}${fraction}`;
  },
};

/// The decoder for one column of a binary (prepared) result.
function myBinaryDecoder(column, temporal) {
  const unsigned = (column.flags & MY_UNSIGNED_FLAG) !== 0;
  const dates = temporal ? MY_TEMPORAL : MY_LEGACY;
  switch (column.type) {
    case MY.TINY:
      return unsigned ? MY_DECODERS.u8 : MY_DECODERS.i8;
    case MY.SHORT:
      return unsigned ? MY_DECODERS.u16 : MY_DECODERS.i16;
    case MY.YEAR:
      return MY_DECODERS.u16;
    case MY.LONG:
    case MY.INT24:
      return unsigned ? MY_DECODERS.u32 : MY_DECODERS.i32;
    case MY.LONGLONG:
      return unsigned ? MY_DECODERS.u64 : MY_DECODERS.i64;
    case MY.FLOAT:
      return MY_DECODERS.f32;
    case MY.DOUBLE:
      return MY_DECODERS.f64;
    case MY.DATE:
      return dates.date;
    case MY.DATETIME:
      return dates.datetime;
    case MY.TIMESTAMP:
      return dates.timestamp;
    case MY.TIME:
      return dates.time;
    case MY.JSON:
      return MY_DECODERS.json;
    // Exact decimals stay text: a number would round them.
    case MY.DECIMAL:
    case MY.NEWDECIMAL:
      return myText;
    case MY.BIT:
    case MY.GEOMETRY:
    case MY.VECTOR:
      return myRaw;
    default:
      // Strings and blobs: the character set says which.
      return column.charset === MY_BINARY_CHARSET ? myRaw : myText;
  }
}

/// Text-protocol values are text; numbers are parsed and bytes kept as bytes.
function myTextDecoder(column) {
  switch (column.type) {
    case MY.TINY:
    case MY.SHORT:
    case MY.LONG:
    case MY.INT24:
    case MY.YEAR:
    case MY.FLOAT:
    case MY.DOUBLE:
      return (bytes, view, start, length) => Number(myText(bytes, view, start, length));
    case MY.LONGLONG:
      return (bytes, view, start, length) => {
        const value = BigInt(myText(bytes, view, start, length));
        return value >= -9007199254740991n && value <= 9007199254740991n ? Number(value) : value;
      };
    case MY.JSON:
      return (bytes, view, start, length) => JSON.parse(myText(bytes, view, start, length));
    default:
      return column.charset === MY_BINARY_CHARSET ? myRaw : myText;
  }
}

// -- parameters -------------------------------------------------------------

const MY_I64_MIN = -(1n << 63n);
const MY_I64_MAX = (1n << 63n) - 1n;
const MY_U64_MAX = (1n << 64n) - 1n;
const MY_NO_PARAMS = new Uint8Array(0);

/// A little-endian writer for the parameter block.
class MyParamWriter {
  constructor(capacity) {
    this.bytes = new Uint8Array(Math.max(capacity, 16));
    this.view = new DataView(this.bytes.buffer);
    this.at = 0;
  }
  _room(n) {
    if (this.at + n <= this.bytes.length) return;
    let size = this.bytes.length * 2;
    while (size < this.at + n) size *= 2;
    const grown = new Uint8Array(size);
    grown.set(this.bytes.subarray(0, this.at));
    this.bytes = grown;
    this.view = new DataView(grown.buffer);
  }
  u8(v) {
    this._room(1);
    this.bytes[this.at++] = v;
    return this;
  }
  u16(v) {
    this._room(2);
    this.view.setUint16(this.at, v, true);
    this.at += 2;
    return this;
  }
  u32(v) {
    this._room(4);
    this.view.setUint32(this.at, v >>> 0, true);
    this.at += 4;
    return this;
  }
  i64(v) {
    this._room(8);
    this.view.setBigInt64(this.at, v, true);
    this.at += 8;
    return this;
  }
  u64(v) {
    this._room(8);
    this.view.setBigUint64(this.at, v, true);
    this.at += 8;
    return this;
  }
  f64(v) {
    this._room(8);
    this.view.setFloat64(this.at, v, true);
    this.at += 8;
    return this;
  }
  raw(bytes) {
    this._room(bytes.length);
    this.bytes.set(bytes, this.at);
    this.at += bytes.length;
    return this;
  }
  lenencBytes(bytes) {
    const n = bytes.length;
    if (n < 0xfb) this.u8(n);
    else if (n <= 0xffff) this.u8(0xfc).u16(n);
    else if (n <= 0xff_ffff) this.u8(0xfd).u16(n & 0xffff).u8(n >>> 16);
    else this.u8(0xfe).u64(BigInt(n));
    return this.raw(bytes);
  }
  finish() {
    return this.bytes.subarray(0, this.at);
  }
}

/// The parameter block of a `COM_STMT_EXECUTE`: the NULL bitmap, the "types
/// follow" flag, a type per parameter, then each non-NULL value. Types go with
/// every execution: a parameter that is a number in one call and a string in
/// the next is ordinary JavaScript.
function encodeMyParams(params) {
  const n = params.length;
  if (n === 0) return MY_NO_PARAMS;
  const bitmap = new Uint8Array((n + 7) >> 3);
  const types = [];
  const encoded = [];
  for (let i = 0; i < n; i++) {
    const param = encodeMyParam(params[i]);
    if (param === null) {
      bitmap[i >> 3] |= 1 << (i & 7);
      types.push(MY.NULL, 0);
    } else {
      types.push(param.type, param.unsigned ? 0x80 : 0);
      encoded.push(param.write);
    }
  }
  const w = new MyParamWriter(64);
  w.raw(bitmap).u8(1);
  for (const byte of types) w.u8(byte);
  for (const write of encoded) write(w);
  return w.finish();
}

function encodeMyParam(value) {
  if (value === null || value === undefined) return null;
  switch (typeof value) {
    case "boolean":
      return { type: MY.TINY, unsigned: false, write: (w) => w.u8(value ? 1 : 0) };
    case "number":
      if (Number.isSafeInteger(value)) {
        return { type: MY.LONGLONG, unsigned: false, write: (w) => w.i64(BigInt(value)) };
      }
      if (!Number.isFinite(value)) throw new TypeError(`MySQL has no representation for ${value}`);
      return { type: MY.DOUBLE, unsigned: false, write: (w) => w.f64(value) };
    case "bigint":
      if (value >= MY_I64_MIN && value <= MY_I64_MAX) {
        return { type: MY.LONGLONG, unsigned: false, write: (w) => w.i64(value) };
      }
      if (value > MY_I64_MAX && value <= MY_U64_MAX) {
        return { type: MY.LONGLONG, unsigned: true, write: (w) => w.u64(value) };
      }
      throw new RangeError(`${value} does not fit in a 64-bit integer`);
    case "string":
      return myStringParam(value);
    default:
      break;
  }
  if (value instanceof Uint8Array) {
    return { type: MY.BLOB, unsigned: false, write: (w) => w.lenencBytes(value) };
  }
  if (ArrayBuffer.isView(value)) {
    const bytes = new Uint8Array(value.buffer, value.byteOffset, value.byteLength);
    return { type: MY.BLOB, unsigned: false, write: (w) => w.lenencBytes(bytes) };
  }
  if (value instanceof ArrayBuffer) {
    const bytes = new Uint8Array(value);
    return { type: MY.BLOB, unsigned: false, write: (w) => w.lenencBytes(bytes) };
  }
  if (value instanceof Date) {
    if (Number.isNaN(value.getTime())) throw new RangeError("an invalid Date cannot be a parameter");
    return myDatetimeParam(MY.DATETIME, {
      year: value.getUTCFullYear(),
      month: value.getUTCMonth() + 1,
      day: value.getUTCDate(),
      hour: value.getUTCHours(),
      minute: value.getUTCMinutes(),
      second: value.getUTCSeconds(),
      micro: value.getUTCMilliseconds() * 1000,
    });
  }
  const temporal = myTemporalParam(value);
  if (temporal !== null) return temporal;
  // Arrays and plain objects are documents: a JSON column validates them.
  return myStringParam(JSON.stringify(value));
}

function myStringParam(value) {
  const bytes = MY_ENCODER.encode(value);
  return { type: MY.VAR_STRING, unsigned: false, write: (w) => w.lenencBytes(bytes) };
}

function myDatetimeParam(type, d) {
  return {
    type,
    unsigned: false,
    write: (w) => {
      w.u8(11).u16(d.year).u8(d.month).u8(d.day).u8(d.hour).u8(d.minute).u8(d.second);
      w.u32(d.micro);
    },
  };
}

/// Temporal values as the MySQL type that means the same thing; an instant
/// goes as its UTC wall time, which the UTC session reads it as.
function myTemporalParam(value) {
  if (typeof Temporal === "undefined") return null;
  if (value instanceof Temporal.Instant) {
    return myTemporalParam(value.toZonedDateTimeISO("UTC").toPlainDateTime());
  }
  if (value instanceof Temporal.ZonedDateTime) return myTemporalParam(value.toInstant());
  if (value instanceof Temporal.PlainDateTime) {
    return myDatetimeParam(MY.DATETIME, {
      year: value.year,
      month: value.month,
      day: value.day,
      hour: value.hour,
      minute: value.minute,
      second: value.second,
      micro: value.millisecond * 1000 + value.microsecond,
    });
  }
  if (value instanceof Temporal.PlainDate) {
    return {
      type: MY.DATE,
      unsigned: false,
      write: (w) => w.u8(4).u16(value.year).u8(value.month).u8(value.day),
    };
  }
  if (value instanceof Temporal.PlainTime) {
    return myTimeParam(false, 0, value.hour, value.minute, value.second, value.millisecond * 1000 + value.microsecond);
  }
  if (value instanceof Temporal.Duration) {
    const total = value.round({ largestUnit: "hour", smallestUnit: "microsecond" });
    const hours = Math.abs(total.hours);
    return myTimeParam(
      total.sign < 0,
      Math.floor(hours / 24),
      hours % 24,
      Math.abs(total.minutes),
      Math.abs(total.seconds),
      Math.abs(total.milliseconds) * 1000 + Math.abs(total.microseconds),
    );
  }
  return null;
}

function myTimeParam(negative, days, hour, minute, second, micro) {
  return {
    type: MY.TIME,
    unsigned: false,
    write: (w) => {
      w.u8(12).u8(negative ? 1 : 0).u32(days).u8(hour).u8(minute).u8(second).u32(micro);
    },
  };
}

// -- errors -------------------------------------------------------------------

/// Server error code → the portable code; the code is the precise one, so it is
/// mapped first, and the SQLSTATE's class answers for codes with no entry.
const MY_BY_CODE = {
  1062: DbErrorCode.UniqueViolation,
  1586: DbErrorCode.UniqueViolation,
  1451: DbErrorCode.ForeignKeyViolation,
  1452: DbErrorCode.ForeignKeyViolation,
  1216: DbErrorCode.ForeignKeyViolation,
  1217: DbErrorCode.ForeignKeyViolation,
  1048: DbErrorCode.NotNullViolation,
  1364: DbErrorCode.NotNullViolation,
  3819: DbErrorCode.CheckViolation,
  4025: DbErrorCode.CheckViolation,
  1213: DbErrorCode.Deadlock,
  1205: DbErrorCode.Busy,
  3572: DbErrorCode.Busy,
  1317: DbErrorCode.Timeout,
  3024: DbErrorCode.Timeout,
  1969: DbErrorCode.Timeout,
  1045: DbErrorCode.AuthFailed,
  1044: DbErrorCode.AuthFailed,
  1251: DbErrorCode.AuthFailed,
  1064: DbErrorCode.Syntax,
  1146: DbErrorCode.UndefinedTable,
  1051: DbErrorCode.UndefinedTable,
  1054: DbErrorCode.UndefinedColumn,
  1290: DbErrorCode.ReadOnly,
  1792: DbErrorCode.ReadOnly,
  1040: DbErrorCode.Throttled,
  1203: DbErrorCode.Throttled,
  1226: DbErrorCode.Throttled,
  1295: DbErrorCode.Unsupported,
  1235: DbErrorCode.Unsupported,
  1053: DbErrorCode.ConnectionLost,
};

function myPortableCode(server) {
  const exact = MY_BY_CODE[server.code];
  if (exact !== undefined) return exact;
  switch (server.sqlstate.slice(0, 2)) {
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

const MY_FAILURE_CODES = {
  lost: DbErrorCode.ConnectionLost,
  auth: DbErrorCode.AuthFailed,
  busy: DbErrorCode.ConnectionBusy,
  unsupported: DbErrorCode.Unsupported,
  mismatch: DbErrorCode.Backend,
};

/// The `DbError` for an op's `{ error }`, keeping everything the server said.
function myError(failure) {
  if (failure.kind === "server") {
    const server = { code: failure.code, sqlstate: failure.sqlstate, message: failure.message };
    const error = asDbError(
      Object.assign(new Error(server.message), { code: String(server.code) }),
      myPortableCode(server),
    );
    return Object.assign(error, { server });
  }
  return new DbError(failure.message, { code: MY_FAILURE_CODES[failure.kind] ?? DbErrorCode.Backend });
}

// -- the connection string -----------------------------------------------------

const MYSQL_DIALECT = new Dialect({
  name: "mysql",
  placeholder: () => "?",
  quote: "`",
  supports: {
    // MariaDB has RETURNING for INSERT and DELETE; MySQL has none. One dialect
    // answers for both, so it is the answer that holds for both.
    returning: false,
    savepoints: true,
    namedParameters: false,
  },
});

/// `MYSQL_HOST`, `MYSQL_TCP_PORT` and `MYSQL_PWD`, which the `mysql` client
/// reads: defaults below the URL and options. Reading them needs `Env`, and is
/// deferred to the first connect for the reason the PostgreSQL defaults are.
async function myEnvironmentDefaults() {
  const options = {};
  try {
    const { env: processEnv, unmask: unmaskSecret } = await import("runtime:process");
    if (processEnv.MYSQL_HOST) options.host = String(processEnv.MYSQL_HOST);
    if (processEnv.MYSQL_TCP_PORT) options.port = Number(processEnv.MYSQL_TCP_PORT);
    if (processEnv.MYSQL_PWD) options.password = String(unmaskSecret(processEnv.MYSQL_PWD));
  } catch {
    return {};
  }
  return options;
}

/// `mysql://user:password@host:port/database?ssl-mode=REQUIRED`. `ssl-mode`
/// takes MySQL's spellings and `sslmode` the PostgreSQL driver's; timeouts are
/// seconds in the URL and milliseconds as options.
function parseMyConnectionString(url, overrides = {}, environment = {}) {
  const parsed = new URL(url);
  const options = {};
  if (parsed.hostname !== "") options.host = decodeURIComponent(parsed.hostname);
  if (parsed.port !== "") options.port = Number(parsed.port);
  const database = decodeURIComponent(parsed.pathname.replace(/^\//, ""));
  if (database !== "") options.database = database;
  if (parsed.username !== "") options.user = decodeURIComponent(parsed.username);
  if (parsed.password !== "") options.password = decodeURIComponent(parsed.password);
  const mode = (parsed.searchParams.get("ssl-mode") ?? parsed.searchParams.get("sslmode") ?? "").toLowerCase();
  if (mode === "disabled" || mode === "disable") options.sslmode = "disable";
  else if (mode === "preferred" || mode === "prefer") options.sslmode = "prefer";
  else if (mode === "required" || mode === "require") options.sslmode = "require";
  const connectSeconds =
    parsed.searchParams.get("connect-timeout") ?? parsed.searchParams.get("connect_timeout");
  if (connectSeconds !== null && connectSeconds !== "") {
    const seconds = Number(connectSeconds);
    if (Number.isFinite(seconds) && seconds >= 0) options.connectTimeout = seconds * 1000;
  }
  const retrieval = parsed.searchParams.get("allowPublicKeyRetrieval");
  if (retrieval !== null) options.allowPublicKeyRetrieval = retrieval === "true";
  // A certificate, not a path: reading a file is not a URL's to do.
  const rootCert = parsed.searchParams.get("ssl-ca") ?? parsed.searchParams.get("sslrootcert");
  if (rootCert !== null && rootCert !== "") options.sslRootCert = rootCert;
  return {
    host: "localhost",
    port: 3306,
    ...environment,
    ...pgDefined(options),
    ...pgDefined(overrides),
  };
}

// -- rows ----------------------------------------------------------------------

/// Row classes, one per shape for the whole process.
const MY_SHAPES = new Map();

function myRowShape(columns, binary, temporal) {
  let key = `${binary ? "B" : "S"}${temporal ? "T" : "L"}`;
  for (const c of columns) {
    key += `|${c.name}\u0000${c.type}.${c.flags & MY_UNSIGNED_FLAG}.${c.charset === MY_BINARY_CHARSET ? 1 : 0}`;
  }
  let shape = MY_SHAPES.get(key);
  if (shape === undefined) {
    shape = defineRowShape(
      columns.map((c) => ({ name: c.name, declType: null, type: c.type })),
      { decoders: columns.map((c) => (binary ? myBinaryDecoder(c, temporal) : myTextDecoder(c))) },
    );
    if (MY_SHAPES.size >= SHAPE_LIMIT) MY_SHAPES.delete(MY_SHAPES.keys().next().value);
  } else {
    MY_SHAPES.delete(key);
  }
  MY_SHAPES.set(key, shape);
  return shape;
}

// -- the connection ------------------------------------------------------------

/// Names a connect in flight, so a timeout can abort it.
let myNextTicket = 1;

class MySqlConnection extends BaseConnection {
  constructor() {
    super({ dialect: MYSQL_DIALECT, backend: "mysql" });
    this._id = null;
    this._fatal = null;
    /// The server's version string, from its greeting.
    this.serverVersion = "";
    /// This connection's id on the server — what `KILL QUERY` names.
    this.connectionId = 0;
    this._status = 0;
    this._temporal = true;
    /// Row classes by the engine's statement key, dropped as it evicts.
    this._shapes = new Map();
    this._streaming = false;
    this._locked = false;
    this._waiters = [];
    this._unlock = () => {
      const next = this._waiters.shift();
      if (next === undefined) this._locked = false;
      else next();
    };
  }

  get usable() {
    return this._fatal === null && this._id !== null && !this._closed;
  }

  /// Fit for the next caller: usable, and not inside a transaction someone else
  /// opened, which would leak into whoever borrowed it next.
  get reusable() {
    return this.usable && (this._status & MY_STATUS_IN_TRANS) === 0;
  }

  /// Whether the server is MariaDB: the same protocol, its own dialect.
  get mariadb() {
    return this.serverVersion.includes("MariaDB");
  }

  async open(options) {
    const budget = options.connectTimeout ?? 10_000;
    const ticket = myNextTicket++;
    const opening = this._connect(options, ticket);
    if (budget <= 0) return opening;
    let timer;
    const expired = new Promise((_, reject) => {
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
      await Promise.race([opening, expired]);
    } catch (e) {
      opening.then(() => this._close()).catch(() => {});
      await ops.my_abort_connect(ticket);
      throw e;
    } finally {
      clearTimeout(timer);
    }
  }

  async _connect(options, ticket) {
    this._temporal = options.temporal !== false;
    const cacheLimit =
      options.preparedStatementCacheSize === undefined
        ? 100
        : Math.max(0, Math.trunc(options.preparedStatementCacheSize));
    const ca =
      options.sslRootCert === undefined
        ? MY_NO_PARAMS
        : typeof options.sslRootCert === "string"
          ? MY_ENCODER.encode(options.sslRootCert)
          : options.sslRootCert;
    const result = await ops.my_connect(
      options.host ?? "localhost",
      options.port ?? 3306,
      options.sslmode ?? "prefer",
      ca,
      options.user ?? "root",
      options.password ?? "",
      options.database === undefined || options.database === "" ? null : options.database,
      options.serverPublicKey ?? null,
      options.allowPublicKeyRetrieval === true,
      cacheLimit,
      options.statementTimeout !== undefined && options.statementTimeout > 0
        ? Math.trunc(options.statementTimeout)
        : null,
      ticket,
    );
    if (result.error !== undefined) throw myError(result.error);
    this._id = result.id;
    this.serverVersion = result.serverVersion;
    this.connectionId = result.connectionId;
    this._status = result.status;
  }

  _absorb(result) {
    if (result.status !== undefined) this._status = result.status;
    if (result.evicted !== undefined) {
      for (const key of result.evicted) this._shapes.delete(key);
    }
    if (result.error !== undefined) {
      if (result.error.kind === "lost") this._fatal = result.error;
      if (result.error.kind !== "busy") this._streaming = false;
    }
  }

  /// The connection for one exchange; a result set still being read is
  /// refused rather than queued, since only its reader can finish it.
  _acquire() {
    if (this._fatal !== null) throw myError(this._fatal);
    if (this._id === null) throw new DbError("the connection is closed", { code: DbErrorCode.Closed });
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
        "MySQL binds parameters by position; pass an array and use ? placeholders (or the sql`` tag)",
        { code: DbErrorCode.Unsupported },
      );
    }
  }

  async _query({ text, positional, named }) {
    this._rejectNamed(named);
    const params = encodeMyParams(positional);
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
      result = await ops.my_query(this._id, text ?? "", params, positional.length, BATCH_BYTES, false);
    } catch (e) {
      release();
      throw e;
    }
    this._absorb(result);
    if (result.error !== undefined) {
      release();
      throw myError(result.error);
    }
    if (result.affectedRows !== undefined) {
      // A statement with no result set, run through `query()`.
      release();
      return new Rows(PG_EMPTY_SOURCE, PG_NO_ROWS_SHAPE);
    }
    let shape = result.statement === 0 ? undefined : this._shapes.get(result.statement);
    if (result.columns !== undefined) {
      const columns = result.columns.map(([name, type, flags, charset]) => ({ name, type, flags, charset }));
      shape = myRowShape(columns, result.binary, this._temporal);
      if (result.statement !== 0) this._shapes.set(result.statement, shape);
    }
    const first = { bytes: result.bytes, rows: result.rows, done: result.done };
    if (first.done) {
      release();
      return new Rows(pgOneBatch(first), shape);
    }
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
          const next = await ops.my_fetch(self._id, maxBytes ?? BATCH_BYTES);
          self._absorb(next);
          if (next.error !== undefined) {
            release();
            throw myError(next.error);
          }
          if (next.done) release();
          return next;
        },
        async close() {
          if (!held) return;
          try {
            const finished = await ops.my_finish(self._id);
            self._absorb(finished);
            if (finished.error !== undefined) throw myError(finished.error);
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
    const params = encodeMyParams(positional);
    const waiting = this._acquire();
    if (waiting !== null) await waiting;
    try {
      const result = await ops.my_query(this._id, text ?? "", params, positional.length, 0, true);
      this._absorb(result);
      if (result.error !== undefined) throw myError(result.error);
      if (result.affectedRows === undefined) return { changes: 0, lastInsertRowid: null };
      return {
        changes: result.affectedRows,
        lastInsertRowid: result.lastInsertId === 0 ? null : result.lastInsertId,
      };
    } finally {
      this._unlock();
    }
  }

  /// A script — several statements — through the text protocol. No
  /// parameters. Unlike PostgreSQL, MySQL does not wrap a script in a
  /// transaction, and DDL commits implicitly. Rows are discarded.
  async executeScript(sql, options = {}) {
    this._open();
    return this._withSignal(options.signal, async () => {
      const results = await this._script(sql);
      return results.map(([changes, last]) => ({ changes, lastInsertRowid: last === 0 ? null : last }));
    });
  }

  async _script(sql) {
    const waiting = this._acquire();
    if (waiting !== null) await waiting;
    try {
      const result = await ops.my_script(this._id, sql);
      this._absorb(result);
      if (result.error !== undefined) throw myError(result.error);
      return result.results;
    } finally {
      this._unlock();
    }
  }

  /// MySQL's spelling of a transaction's statements, which differs from the
  /// standard's at RELEASE SAVEPOINT, sent through the text protocol.
  async _beginTransaction({ nested, name }) {
    await this._script(nested ? `SAVEPOINT ${name}` : "BEGIN");
  }

  async _commitTransaction({ nested, name }) {
    await this._script(nested ? `RELEASE SAVEPOINT ${name}` : "COMMIT");
  }

  async _rollbackTransaction({ nested, name }) {
    await this._script(nested ? `ROLLBACK TO SAVEPOINT ${name}` : "ROLLBACK");
  }

  async _cancel() {
    await this.cancel();
  }

  /// Stops the statement this connection is running, if there is one: a second
  /// connection, as the same user, runs `KILL QUERY` naming this one. The
  /// statement fails with ER_QUERY_INTERRUPTED; this connection stays usable.
  async cancel() {
    if (this._fatal !== null || this._id === null) return;
    const result = await ops.my_cancel(this._id);
    if (result?.error !== undefined) throw myError(result.error);
  }

  /// Asks the server whether it is still there.
  async ping() {
    this._open();
    const waiting = this._acquire();
    if (waiting !== null) await waiting;
    try {
      const result = await ops.my_ping(this._id);
      if (result?.error !== undefined) {
        this._absorb(result);
        throw myError(result.error);
      }
    } finally {
      this._unlock();
    }
  }

  async _close() {
    const id = this._id;
    if (id === null) return;
    this._id = null;
    await ops.my_close(id);
  }
}

/// A pool of MySQL connections, presenting the surface one connection does,
/// plus `executeScript` on a borrowed connection.
class MySqlPooled extends PooledConnection {
  executeScript(sql, options = {}) {
    return this.withConnection((connection) => connection.executeScript(sql, options));
  }
}

/// The built-in MySQL driver: `mysql:` and `mariadb:` URLs.
const mysql = defineDriver({
  name: "mysql",
  schemes: ["mysql", "mariadb"],
  dialect: MYSQL_DIALECT,
  async open(url, options = {}) {
    const connection = new MySqlConnection();
    await connection.open(parseMyConnectionString(url, options, await myEnvironmentDefaults()));
    return connection;
  },
  /// Nothing is opened here; each connection reads `MYSQL_*` as it opens.
  pooled(url, options = {}, poolOptions = {}) {
    parseMyConnectionString(url, options);
    return new MySqlPooled(mysql, url, options, poolOptions);
  },
});

export { mysql };

// The module's default export, here because it names every built-in driver
// and this file is appended last.
export default { connect, sql, queryAst, sqlite, postgres, mysql, defineDriver, DbError, DbErrorCode };
