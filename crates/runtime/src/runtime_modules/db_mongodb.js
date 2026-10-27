
// ---------------------------------------------------------------------------
// MongoDB (DECISIONS D186)
// ---------------------------------------------------------------------------
//
// The protocol, the authentication and the pool are Rust's (`mongo_ops`); this
// is the connection string, the collection API, and turning replies into
// values and errors. Commands and replies are BSON, encoded and decoded by the
// engine (D187). A command is one op: the pool picks a connection inside it.

import { BSON as MongoBSON } from "runtime:serialization";

const MONGO_DIALECT = new Dialect({
  name: "mongodb",
  placeholder: () => {
    throw dbError(
      "MongoDB has no placeholders: a command is a document",
      DbErrorCode.QueryForm,
    );
  },
  // Neither SQL text nor a query AST: MongoDB is reached through db() and
  // collection(). Transactions need a replica set, which comes later.
  supports: { queryText: false, queryAst: false, transactions: false },
});

const MONGO_DEFAULT_PORT = 27017;
const MONGO_DEFAULT_POOL = 100;
const MONGO_DEFAULT_CONNECT_TIMEOUT = 30_000;
// The room an insert's command and framing take beside its documents.
const MONGO_COMMAND_OVERHEAD = 16 * 1024;

// Every command and connect has a ticket, so an abort can find it in the pool.
let mongoTicket = 1;

// -- errors -------------------------------------------------------------------

const MONGO_CODES = new Map([
  [11000, DbErrorCode.UniqueViolation],
  [11001, DbErrorCode.UniqueViolation],
  [12582, DbErrorCode.UniqueViolation],
  [18, DbErrorCode.AuthFailed],
  [50, DbErrorCode.Timeout],
  [262, DbErrorCode.Timeout],
  [26, DbErrorCode.NotFound],
  [112, DbErrorCode.SerializationFailure],
  [121, DbErrorCode.CheckViolation],
]);

/// A server error document (`ok: 0`, a write error, a write-concern error) as a
/// DbError that keeps what the server said on `server`.
function mongoServerError(doc) {
  const code = typeof doc.code === "number" ? doc.code : 0;
  const error = new DbError(doc.errmsg ?? `the server returned error ${code}`, {
    code: MONGO_CODES.get(code) ?? DbErrorCode.Backend,
    backendCode: code,
  });
  error.server = {
    code,
    codeName: doc.codeName ?? "",
    errorLabels: doc.errorLabels ?? [],
  };
  if (doc.errInfo !== undefined) error.server.errInfo = doc.errInfo;
  if (doc.keyValue !== undefined) error.server.keyValue = doc.keyValue;
  return error;
}

const MONGO_FAILURES = {
  lost: DbErrorCode.ConnectionLost,
  auth: DbErrorCode.AuthFailed,
  unsupported: DbErrorCode.Unsupported,
  closed: DbErrorCode.Closed,
};

function mongoFailure(failure) {
  if (failure.kind === "server") {
    return mongoServerError({ code: failure.code, codeName: failure.codeName, errmsg: failure.message });
  }
  return new DbError(failure.message, { code: MONGO_FAILURES[failure.kind] ?? DbErrorCode.Backend });
}

// -- the connection string ------------------------------------------------------

// Each option this driver takes, by its lowercase spelling (URI options are
// case-insensitive). Anything else is refused by name: a misspelled option that
// was quietly ignored is a setting the caller believes they have.
const MONGO_OPTION_NAMES = new Map(
  [
    "appName",
    "authMechanism",
    "authSource",
    "compressors",
    "connectTimeoutMS",
    "directConnection",
    "journal",
    "maxPoolSize",
    "readConcernLevel",
    "readPreference",
    "replicaSet",
    "retryReads",
    "retryWrites",
    "serverSelectionTimeoutMS",
    "ssl",
    "timeoutMS",
    "tls",
    "tlsAllowInvalidCertificates",
    "tlsAllowInvalidHostnames",
    "tlsCAFile",
    "tlsInsecure",
    "w",
    "wtimeoutMS",
  ].map((name) => [name.toLowerCase(), name]),
);

function mongoBad(message) {
  return dbError(message, DbErrorCode.Unsupported);
}

function mongoBoolean(name, value) {
  if (typeof value === "boolean") return value;
  if (value === "true") return true;
  if (value === "false") return false;
  throw mongoBad(`${name} must be true or false, not ${value}`);
}

function mongoCount(name, value) {
  const n = typeof value === "number" ? value : /^\d+$/.test(String(value)) ? Number(value) : NaN;
  if (!Number.isSafeInteger(n) || n < 0) throw mongoBad(`${name} must be a whole number, not ${value}`);
  return n;
}

/// The settings for a client: `mongodb://` URL first, then `options` over it.
function parseMongoConnectionString(url, options = {}) {
  const lower = url.toLowerCase();
  if (lower.startsWith("mongodb+srv:")) {
    throw mongoBad("mongodb+srv:// is not supported yet; connect with mongodb://host:port");
  }
  if (!lower.startsWith("mongodb://")) {
    throw mongoBad(`${url} is not a mongodb:// connection string`);
  }
  let rest = url.slice("mongodb://".length);
  let query = "";
  const q = rest.indexOf("?");
  if (q >= 0) {
    query = rest.slice(q + 1);
    rest = rest.slice(0, q);
  }
  const at = rest.lastIndexOf("@");
  const userinfo = at >= 0 ? rest.slice(0, at) : null;
  const slash = rest.indexOf("/", at + 1);
  const hostList = slash >= 0 ? rest.slice(at + 1, slash) : rest.slice(at + 1);
  const database = slash >= 0 && slash + 1 < rest.length ? decodeURIComponent(rest.slice(slash + 1)) : null;

  const settings = {
    host: "",
    port: MONGO_DEFAULT_PORT,
    user: null,
    password: null,
    database,
    tls: false,
    maxPoolSize: MONGO_DEFAULT_POOL,
    connectTimeoutMS: MONGO_DEFAULT_CONNECT_TIMEOUT,
  };

  if (userinfo !== null) {
    const colon = userinfo.indexOf(":");
    const user = colon >= 0 ? userinfo.slice(0, colon) : userinfo;
    const password = colon >= 0 ? userinfo.slice(colon + 1) : null;
    // The specification requires these escaped, so an unescaped one is a URL
    // that was split somewhere the author did not mean.
    if (/[:/@]/.test(user) || (password !== null && /[:/@]/.test(password))) {
      throw mongoBad("a username or password must percent-encode ':', '/' and '@'");
    }
    settings.user = decodeURIComponent(user);
    settings.password = password === null ? null : decodeURIComponent(password);
  }

  const hosts = hostList.split(",");
  if (hosts.length !== 1) {
    throw mongoBad(
      "this driver connects to one server; replica sets and sharded clusters are not supported yet",
    );
  }
  const host = hosts[0];
  if (host === "") throw mongoBad(`${url} names no host`);
  if (/%2f|\.sock$/i.test(host)) throw mongoBad("Unix-socket connections are not supported yet");
  const ipv6 = /^\[([^\]]+)\](?::(\d+))?$/.exec(host);
  const named = ipv6 ? null : /^([^:]+)(?::(\d+))?$/.exec(host);
  const parts = ipv6 ?? named;
  if (!parts) throw mongoBad(`${host} is not a host and port`);
  settings.host = parts[1];
  if (parts[2] !== undefined) {
    const port = Number(parts[2]);
    if (!(port >= 1 && port <= 65535)) throw mongoBad(`${parts[2]} is not a port`);
    settings.port = port;
  }

  const given = {};
  for (const pair of query.split("&")) {
    if (pair === "") continue;
    const eq = pair.indexOf("=");
    const key = decodeURIComponent(eq >= 0 ? pair.slice(0, eq) : pair);
    const value = eq >= 0 ? decodeURIComponent(pair.slice(eq + 1)) : "";
    given[key] = value;
  }
  // Options passed to connect() win over the URL's.
  const { auth, ca, ...rest2 } = options;
  Object.assign(given, rest2);
  if (auth !== undefined) {
    if (auth.username !== undefined) settings.user = String(auth.username);
    if (auth.password !== undefined) settings.password = String(auth.password);
  }
  if (ca !== undefined) settings.ca = ca;

  for (const [key, value] of Object.entries(given)) {
    const name = MONGO_OPTION_NAMES.get(key.toLowerCase());
    if (name === undefined) throw mongoBad(`the MongoDB driver does not know the option ${key}`);
    mongoOption(settings, name, value);
  }
  settings.authSource ??= settings.database ?? "admin";
  return settings;
}

function mongoOption(settings, name, value) {
  switch (name) {
    case "appName":
      settings.appName = String(value);
      return;
    case "authMechanism": {
      const mechanism = String(value).toUpperCase();
      if (mechanism !== "SCRAM-SHA-1" && mechanism !== "SCRAM-SHA-256") {
        throw mongoBad(`the ${value} authentication mechanism is not supported yet`);
      }
      settings.authMechanism = mechanism;
      return;
    }
    case "authSource":
      settings.authSource = String(value);
      return;
    case "tls":
    case "ssl": {
      const tls = mongoBoolean(name, value);
      if (settings.tlsGiven !== undefined && settings.tlsGiven !== tls) {
        throw mongoBad("tls and ssl disagree");
      }
      settings.tlsGiven = tls;
      settings.tls = tls;
      return;
    }
    case "tlsCAFile":
      settings.tlsCAFile = String(value);
      settings.tls = settings.tlsGiven ?? true;
      return;
    case "tlsInsecure":
    case "tlsAllowInvalidCertificates":
    case "tlsAllowInvalidHostnames":
      if (mongoBoolean(name, value)) {
        throw mongoBad(
          `${name} is not supported: esrun does not turn certificate checks off. Trust a private CA with tlsCAFile or the ca option`,
        );
      }
      return;
    case "maxPoolSize": {
      // 0 means no limit, as the specification spells it.
      const n = mongoCount(name, value);
      settings.maxPoolSize = n === 0 ? Number.MAX_SAFE_INTEGER : n;
      return;
    }
    case "connectTimeoutMS":
      settings.connectTimeoutMS = mongoCount(name, value);
      return;
    case "timeoutMS":
      settings.timeoutMS = mongoCount(name, value);
      return;
    case "serverSelectionTimeoutMS":
      // One server: there is nothing to select between, so the connect timeout
      // is what bounds reaching it.
      mongoCount(name, value);
      return;
    case "directConnection":
      mongoBoolean(name, value);
      return;
    case "replicaSet":
      throw mongoBad(
        "replica sets are not supported yet; connect to one member, with directConnection=true",
      );
    case "readPreference":
      if (String(value) !== "primary") {
        throw mongoBad(`readPreference ${value} needs replica-set support, which is not available yet`);
      }
      return;
    case "retryReads":
    case "retryWrites":
      // Accepted so that Atlas's connection strings work; nothing is retried
      // yet (see the MongoDB page).
      mongoBoolean(name, value);
      return;
    case "compressors":
      // Compression is not negotiated yet; a server talks uncompressed to a
      // client that offers none, as the specification allows.
      return;
    case "w": {
      const w = /^\d+$/.test(String(value)) ? Number(value) : String(value);
      settings.writeConcern = { ...settings.writeConcern, w };
      return;
    }
    case "journal":
      settings.writeConcern = { ...settings.writeConcern, j: mongoBoolean(name, value) };
      return;
    case "wtimeoutMS":
      settings.writeConcern = { ...settings.writeConcern, wtimeout: mongoCount(name, value) };
      return;
    case "readConcernLevel":
      settings.readConcern = { level: String(value) };
      return;
  }
}

async function openMongo(url, options = {}) {
  const settings = parseMongoConnectionString(url, options);
  let ca = new Uint8Array(0);
  if (settings.ca !== undefined) {
    ca = typeof settings.ca === "string" ? new TextEncoder().encode(settings.ca) : settings.ca;
  } else if (settings.tlsCAFile !== undefined) {
    // Read through runtime:fs, so the file needs a read grant like any other.
    const { file } = await import("runtime:fs");
    ca = await file(settings.tlsCAFile).bytes();
  }
  const ticket = mongoTicket++;
  let timedOut = false;
  const timer =
    settings.connectTimeoutMS > 0
      ? setTimeout(() => {
          timedOut = true;
          ops.mongo_abort_connect(ticket).catch(() => {});
        }, settings.connectTimeoutMS)
      : null;
  let result;
  try {
    const password = settings.password ?? "";
    result = await ops.mongo_connect(
      settings.host,
      settings.port,
      settings.tls,
      ca,
      settings.user,
      password,
      password.normalize("NFKC"),
      settings.authSource,
      settings.authMechanism ?? null,
      settings.appName ?? null,
      settings.maxPoolSize,
      ticket,
    );
  } finally {
    if (timer !== null) clearTimeout(timer);
  }
  if (result.error !== undefined) {
    if (timedOut) {
      throw dbError(
        `connecting to ${settings.host}:${settings.port} took longer than connectTimeoutMS (${settings.connectTimeoutMS} ms)`,
        DbErrorCode.Timeout,
      );
    }
    throw mongoFailure(result.error);
  }
  return new MongoConnection(result, settings);
}

// -- sessions -------------------------------------------------------------------
//
// The driver sessions specification. A server session is an `lsid` the driver
// makes up (a random UUID) and the server learns of on first use; a client keeps
// the ones it has finished with, newest first, and reuses them. Every command
// runs in a session: the caller's, or an implicit one held for that command (or
// for a cursor, until it is exhausted or closed).

// A server session this close to the server's timeout is not handed out again.
const MONGO_SESSION_MARGIN_MS = 60_000;

class MongoServerSessionPool {
  constructor() {
    this._queue = [];
  }

  #expiring(session, timeoutMinutes) {
    return Date.now() - session.lastUse > timeoutMinutes * 60_000 - MONGO_SESSION_MARGIN_MS;
  }

  acquire(timeoutMinutes) {
    while (this._queue.length > 0) {
      const session = this._queue.shift();
      if (!this.#expiring(session, timeoutMinutes)) return session;
    }
    return { id: { id: new MongoBSON.UUID() }, lastUse: Date.now(), dirty: false, txnNumber: 0n };
  }

  release(session, timeoutMinutes) {
    while (this._queue.length > 0 && this.#expiring(this._queue.at(-1), timeoutMinutes)) this._queue.pop();
    // One that saw a network error may be in an unknown state on the server.
    if (session.dirty || this.#expiring(session, timeoutMinutes)) return;
    this._queue.unshift(session);
  }

  drain() {
    return this._queue.splice(0).map((session) => session.id);
  }
}

function mongoTimestampAfter(a, b) {
  if (b == null) return a != null;
  if (a == null) return false;
  return a.t > b.t || (a.t === b.t && a.i > b.i);
}

/// A session: a sequence of operations the server can relate. From
/// `client.startSession()`; pass it as `{ session }` to any operation.
class MongoSession {
  constructor(client, { causalConsistency = true } = {}, implicit = false) {
    this.client = client;
    this.explicit = !implicit;
    this.causalConsistency = implicit ? false : causalConsistency !== false;
    this.clusterTime = null;
    this.operationTime = null;
    this.hasEnded = false;
    this._server = null;
  }

  /// The session id the server knows it by.
  get id() {
    return this._serverSession().id;
  }

  _serverSession() {
    if (this.hasEnded) throw mongoBad("the session has ended");
    this._server ??= this.client._sessions.acquire(this.client._sessionTimeout);
    return this._server;
  }

  /// Takes a `$clusterTime` from elsewhere, when it is later than this one.
  advanceClusterTime(clusterTime) {
    if (mongoTimestampAfter(clusterTime?.clusterTime, this.clusterTime?.clusterTime)) {
      this.clusterTime = clusterTime;
    }
  }

  /// Takes an operation time from elsewhere, when it is later than this one.
  advanceOperationTime(operationTime) {
    if (mongoTimestampAfter(operationTime, this.operationTime)) this.operationTime = operationTime;
  }

  /// Ends the session and returns its server session for reuse. Ending twice
  /// does nothing.
  async endSession() {
    if (this.hasEnded) return;
    this.hasEnded = true;
    if (this._server !== null) {
      this.client._sessions.release(this._server, this.client._sessionTimeout);
      this._server = null;
    }
  }

  async [Symbol.asyncDispose]() {
    await this.endSession();
  }
}

// Reads that wait for an explicit causally consistent session's last write.
const MONGO_CAUSAL_READS = new Set(["find", "aggregate", "count", "distinct"]);

// -- the client -----------------------------------------------------------------

/// A client: a pool of connections to one server, opened by connect().
class MongoConnection extends BaseConnection {
  constructor(result, settings) {
    super({ dialect: MONGO_DIALECT, backend: "mongodb" });
    this._id = result.id;
    this._settings = settings;
    const hello = MongoBSON.decode(result.hello);
    this._maxMessageSize = result.maxMessageSize;
    this._maxBsonObjectSize = Number(hello.maxBsonObjectSize ?? 16 * 1024 * 1024);
    this._maxWriteBatchSize = Number(hello.maxWriteBatchSize ?? 100_000);
    // Present when the server has sessions; every server this driver speaks
    // to does, but a standalone can be configured without.
    this._sessionTimeout =
      hello.logicalSessionTimeoutMinutes == null ? null : Number(hello.logicalSessionTimeoutMinutes);
    this._sessions = new MongoServerSessionPool();
    this._clusterTime = null;
  }

  /// Starts an explicit session. Pass it as `{ session }` to the operations it
  /// groups, and end it with `endSession()` or `await using`. A session not
  /// used within a minute or so of starting may already be stale on the server.
  startSession(options = {}) {
    this._open();
    if (this._sessionTimeout === null) {
      throw mongoBad("this server does not support sessions");
    }
    return new MongoSession(this, options);
  }

  /// The database `name`, or the connection string's (`test` when it names
  /// none).
  db(name = this._settings.database ?? "test") {
    this._open();
    return new MongoDb(this, name);
  }

  /// Runs a command against `options.db` (default `admin`) and returns its reply.
  command(command, options = {}) {
    return this.db(options.db ?? "admin").command(command, options);
  }

  async ping(options = {}) {
    await this.command({ ping: 1 }, options);
  }

  async transaction() {
    throw dbError(
      "MongoDB transactions need a replica set, which this driver does not connect to yet",
      DbErrorCode.Unsupported,
    );
  }

  async _close() {
    // Tell the server the pooled sessions are finished with, as the sessions
    // specification asks; a failure here changes nothing for the caller.
    const ids = this._sessions.drain();
    for (let i = 0; i < ids.length; i += 10_000) {
      await this._run("admin", { endSessions: ids.slice(i, i + 10_000) }, { _noSession: true }, null, true).catch(() => {});
    }
    await ops.mongo_close(this._id);
  }

  /// The session a command runs in: the caller's, a cursor's, or a new implicit
  /// one (`owned`, ended after the command). `null` for an unacknowledged write,
  /// which must not carry a session, or a server without sessions.
  _sessionFor(options, unacknowledged) {
    const given = options.session;
    if (given !== undefined) {
      if (!(given instanceof MongoSession) || given.client !== this) {
        throw mongoBad("the session belongs to another client");
      }
      if (given.hasEnded) throw mongoBad("the session has ended");
      if (unacknowledged) throw mongoBad("an unacknowledged write (w: 0) cannot use a session");
      return { session: given, owned: false };
    }
    if (options._cursorSession !== undefined) return { session: options._cursorSession, owned: false };
    if (options._noSession || unacknowledged || this._sessionTimeout === null) return { session: null, owned: false };
    return { session: new MongoSession(this, {}, true), owned: true };
  }

  _gossip(reply, session) {
    const clusterTime = reply.$clusterTime;
    if (clusterTime !== undefined && mongoTimestampAfter(clusterTime.clusterTime, this._clusterTime?.clusterTime)) {
      this._clusterTime = clusterTime;
    }
    if (session !== null) {
      if (clusterTime !== undefined) session.advanceClusterTime(clusterTime);
      if (session.causalConsistency && reply.operationTime !== undefined) {
        session.advanceOperationTime(reply.operationTime);
      }
    }
  }

  /// One command on database `dbName`. `sequence` carries an insert's,
  /// update's or delete's statements as their own encoded documents.
  async _run(dbName, command, options = {}, sequence = null, closing = false) {
    if (!closing) this._open();
    const signal = options.signal;
    if (signal?.aborted) throw signal.reason;
    const timeoutMS = options.timeoutMS ?? this._settings.timeoutMS;
    const body = { ...command };
    const name = Object.keys(command)[0];
    // The server stops the work at the same deadline the client gives up at.
    if (timeoutMS > 0 && body.maxTimeMS === undefined && name !== "getMore" && name !== "killCursors") {
      body.maxTimeMS = timeoutMS;
    }
    const unacknowledged = command.writeConcern?.w === 0;
    const { session, owned } = this._sessionFor(options, unacknowledged);
    try {
      return await this.#send(dbName, name, body, session, options, timeoutMS, signal, sequence);
    } finally {
      if (owned) await session.endSession();
    }
  }

  async #send(dbName, name, body, session, options, timeoutMS, signal, sequence) {
    let server = null;
    if (session !== null && name !== "killCursors") {
      server = session._serverSession();
      body.lsid = server.id;
      server.lastUse = Date.now();
      if (
        session.explicit &&
        session.causalConsistency &&
        session.operationTime !== null &&
        MONGO_CAUSAL_READS.has(name)
      ) {
        body.readConcern = { ...body.readConcern, afterClusterTime: session.operationTime };
      }
    }
    const clusterTime = mongoTimestampAfter(session?.clusterTime?.clusterTime, this._clusterTime?.clusterTime)
      ? session.clusterTime
      : this._clusterTime;
    if (clusterTime !== null) body.$clusterTime = clusterTime;
    body.$db = dbName;
    const bytes = MongoBSON.encode(body);

    const ticket = mongoTicket++;
    let stopped = null;
    const stop = (reason) => {
      if (stopped !== null) return;
      stopped = reason;
      ops.mongo_cancel(this._id, ticket).catch(() => {});
    };
    const onAbort = () => stop(signal.reason);
    signal?.addEventListener("abort", onAbort, { once: true });
    const timer =
      timeoutMS > 0
        ? setTimeout(
            () => stop(dbError(`the operation took longer than timeoutMS (${timeoutMS} ms)`, DbErrorCode.Timeout)),
            timeoutMS,
          )
        : null;
    let reply;
    try {
      reply = await ops.mongo_command(
        this._id,
        bytes,
        ticket,
        sequence?.name ?? null,
        sequence?.documents ?? null,
      );
    } finally {
      if (timer !== null) clearTimeout(timer);
      signal?.removeEventListener("abort", onAbort);
    }
    // A command stopped mid-flight or lost with its connection may have left
    // the server session in an unknown state, so it is not reused.
    if (server !== null && (stopped !== null || (!(reply instanceof Uint8Array) && reply.error.kind === "lost"))) {
      server.dirty = true;
    }
    // Stopped while it ran is stopped, however the command ended.
    if (stopped !== null) throw stopped;
    if (!(reply instanceof Uint8Array)) throw mongoFailure(reply.error);
    const doc = MongoBSON.decode(reply);
    this._gossip(doc, session);
    if (doc.ok !== 1) throw mongoServerError(doc);
    return doc;
  }
}

function mongoName(kind, name) {
  if (typeof name !== "string" || name === "" || name.includes("\0")) {
    throw mongoBad(`a ${kind} name must be a non-empty string`);
  }
  return name;
}

/// A database.
class MongoDb {
  constructor(client, name) {
    this.client = client;
    this.databaseName = mongoName("database", name);
  }

  collection(name) {
    return new MongoCollection(this, mongoName("collection", name));
  }

  /// Runs a command on this database and returns its reply. A reply with
  /// `ok: 0` is thrown as a DbError.
  command(command, options = {}) {
    if (command === null || typeof command !== "object" || Array.isArray(command)) {
      throw new TypeError("a command is a document, e.g. { ping: 1 }");
    }
    return this.client._run(this.databaseName, command, options);
  }

  /// A database-level aggregation (`$currentOp`, `$documents`, …).
  aggregate(pipeline = [], options = {}) {
    return new MongoCursor(this, null, () => mongoAggregateCommand(1, pipeline, options, this.client), options);
  }

  listCollections(filter = {}, options = {}) {
    return new MongoCursor(
      this,
      null,
      () => ({
        listCollections: 1,
        filter,
        ...mongoPick(options, ["nameOnly", "authorizedCollections", "comment"]),
        cursor: options.batchSize === undefined ? {} : { batchSize: options.batchSize },
      }),
      options,
    );
  }

  async createCollection(name, options = {}) {
    const { signal, timeoutMS, ...rest } = options;
    await this.command({ create: mongoName("collection", name), ...rest }, { signal, timeoutMS });
    return this.collection(name);
  }

  dropCollection(name, options = {}) {
    return this.collection(name).drop(options);
  }

  async dropDatabase(options = {}) {
    await this.command(
      { dropDatabase: 1, ...mongoWriteConcern(this.client, options) },
      options,
    );
    return true;
  }
}

function mongoPick(options, keys) {
  const out = {};
  for (const key of keys) if (options[key] !== undefined) out[key] = options[key];
  return out;
}

function mongoWriteConcern(client, options) {
  const writeConcern = options.writeConcern ?? client._settings.writeConcern;
  return writeConcern === undefined ? {} : { writeConcern };
}

function mongoReadConcern(client, options) {
  const readConcern = options.readConcern ?? client._settings.readConcern;
  return readConcern === undefined ? {} : { readConcern };
}

function mongoAggregateCommand(target, pipeline, options, client) {
  if (!Array.isArray(pipeline)) throw new TypeError("a pipeline is an array of stages");
  const last = pipeline.at(-1);
  const writes = last !== undefined && ("$out" in last || "$merge" in last);
  return {
    aggregate: target,
    pipeline,
    cursor: options.batchSize === undefined ? {} : { batchSize: options.batchSize },
    ...mongoPick(options, ["allowDiskUse", "hint", "comment", "maxTimeMS", "collation", "let", "bypassDocumentValidation"]),
    ...(writes ? mongoWriteConcern(client, options) : mongoReadConcern(client, options)),
  };
}

// -- writes ---------------------------------------------------------------------

/// Sends `statements` (documents, update or delete statements) as one or more
/// commands, each within the server's batch count and message size, and sums
/// what the server reports. An ordered write stops at the first batch with an
/// error; the errors' `index` is into `statements`.
async function mongoWrite(collection, kind, field, statements, options, ordered) {
  const client = collection.db.client;
  const limit = client._maxBsonObjectSize + (kind === "insert" ? 0 : MONGO_COMMAND_OVERHEAD);
  const encoded = statements.map((statement) => {
    const bytes = MongoBSON.encode(statement);
    if (bytes.length > limit) {
      throw dbError(
        `a ${kind === "insert" ? "document" : "statement"} of ${bytes.length} bytes is larger than the server's ${client._maxBsonObjectSize}-byte limit`,
        DbErrorCode.Backend,
        { backendCode: 10334 },
      );
    }
    return bytes;
  });
  const room = client._maxMessageSize - MONGO_COMMAND_OVERHEAD;
  const extra = {
    ordered,
    ...mongoPick(options, ["bypassDocumentValidation", "comment", "let"]),
    ...mongoWriteConcern(client, options),
  };
  const result = { n: 0, nModified: 0, upserted: [], writeErrors: [], writeConcernError: null };
  let start = 0;
  while (start < encoded.length) {
    let end = start;
    let size = 0;
    while (
      end < encoded.length &&
      end - start < client._maxWriteBatchSize &&
      (end === start || size + encoded[end].length <= room)
    ) {
      size += encoded[end].length;
      end++;
    }
    const reply = await collection._run(
      { [kind]: collection.collectionName, ...extra },
      options,
      { name: field, documents: encoded.slice(start, end) },
    );
    result.n += Number(reply.n ?? 0);
    result.nModified += Number(reply.nModified ?? 0);
    for (const u of reply.upserted ?? []) result.upserted.push({ index: u.index + start, _id: u._id });
    for (const e of reply.writeErrors ?? []) result.writeErrors.push({ ...e, index: e.index + start });
    if (reply.writeConcernError) result.writeConcernError = reply.writeConcernError;
    if (ordered && reply.writeErrors?.length) break;
    start = end;
  }
  return result;
}

/// The first write error, or the write-concern error, thrown with everything
/// the server reported and the partial `result`.
function mongoThrowWrite(written, result) {
  const first = written.writeErrors[0] ?? written.writeConcernError;
  if (!first) return;
  const error = mongoServerError(first);
  error.writeErrors = written.writeErrors;
  if (written.writeConcernError) error.writeConcernError = written.writeConcernError;
  error.result = result;
  throw error;
}

function mongoDocument(value, what) {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new TypeError(`${what} must be a document`);
  }
  return value;
}

function mongoWithId(document) {
  mongoDocument(document, "a document to insert");
  // A new object rather than the caller's, given an _id: inserting does not
  // change what was passed in. The id comes back as insertedId.
  return document._id === undefined ? { _id: new MongoBSON.ObjectId(), ...document } : document;
}

function mongoUpdateDocument(update) {
  if (Array.isArray(update)) return update;
  mongoDocument(update, "an update");
  const keys = Object.keys(update);
  if (keys.length === 0 || !keys[0].startsWith("$")) {
    throw new TypeError("an update must use update operators ($set, $inc, …) or be a pipeline; to replace a document use replaceOne");
  }
  return update;
}

function mongoReplacement(replacement) {
  mongoDocument(replacement, "a replacement");
  if (Object.keys(replacement).some((key) => key.startsWith("$"))) {
    throw new TypeError("a replacement cannot contain update operators; to change fields use updateOne");
  }
  return replacement;
}

function mongoUpdateStatement(filter, update, multi, options) {
  return {
    q: mongoDocument(filter, "a filter"),
    u: update,
    multi,
    upsert: options.upsert === true,
    ...mongoPick(options, ["arrayFilters", "hint", "collation"]),
  };
}

function mongoUpdateResult(written) {
  const upserted = written.upserted.length;
  return {
    acknowledged: true,
    matchedCount: written.n - upserted,
    modifiedCount: written.nModified,
    upsertedCount: upserted,
    upsertedId: written.upserted[0]?._id ?? null,
  };
}

// -- collections ----------------------------------------------------------------

/// A collection.
class MongoCollection {
  constructor(db, name) {
    this.db = db;
    this.collectionName = name;
  }

  get namespace() {
    return `${this.db.databaseName}.${this.collectionName}`;
  }

  _run(command, options, sequence) {
    return this.db.client._run(this.db.databaseName, command, options, sequence);
  }

  async insertOne(document, options = {}) {
    const doc = mongoWithId(document);
    const written = await mongoWrite(this, "insert", "documents", [doc], options, true);
    mongoThrowWrite(written, { insertedCount: written.n });
    return { acknowledged: true, insertedId: doc._id };
  }

  async insertMany(documents, options = {}) {
    if (!Array.isArray(documents)) throw new TypeError("insertMany takes an array of documents");
    const docs = documents.map(mongoWithId);
    const ordered = options.ordered !== false;
    const written = await mongoWrite(this, "insert", "documents", docs, options, ordered);
    const failed = new Set(written.writeErrors.map((e) => e.index));
    const insertedIds = {};
    const stop = ordered && written.writeErrors.length > 0 ? written.writeErrors[0].index : docs.length;
    for (let i = 0; i < stop; i++) if (!failed.has(i)) insertedIds[i] = docs[i]._id;
    const result = { acknowledged: true, insertedCount: written.n, insertedIds };
    mongoThrowWrite(written, result);
    return result;
  }

  async updateOne(filter, update, options = {}) {
    const statement = mongoUpdateStatement(filter, mongoUpdateDocument(update), false, options);
    const written = await mongoWrite(this, "update", "updates", [statement], options, true);
    const result = mongoUpdateResult(written);
    mongoThrowWrite(written, result);
    return result;
  }

  async updateMany(filter, update, options = {}) {
    const statement = mongoUpdateStatement(filter, mongoUpdateDocument(update), true, options);
    const written = await mongoWrite(this, "update", "updates", [statement], options, true);
    const result = mongoUpdateResult(written);
    mongoThrowWrite(written, result);
    return result;
  }

  async replaceOne(filter, replacement, options = {}) {
    const statement = mongoUpdateStatement(filter, mongoReplacement(replacement), false, options);
    const written = await mongoWrite(this, "update", "updates", [statement], options, true);
    const result = mongoUpdateResult(written);
    mongoThrowWrite(written, result);
    return result;
  }

  async deleteOne(filter, options = {}) {
    return this.#delete(filter, 1, options);
  }

  async deleteMany(filter, options = {}) {
    return this.#delete(filter, 0, options);
  }

  async #delete(filter, limit, options) {
    const statement = {
      q: mongoDocument(filter, "a filter"),
      limit,
      ...mongoPick(options, ["hint", "collation"]),
    };
    const written = await mongoWrite(this, "delete", "deletes", [statement], options, true);
    const result = { acknowledged: true, deletedCount: written.n };
    mongoThrowWrite(written, result);
    return result;
  }

  /// Several writes, in as few commands as their kinds allow. Ordered (the
  /// default) runs them in order and stops at the first error; unordered may
  /// group them by kind and runs them all.
  async bulkWrite(operations, options = {}) {
    if (!Array.isArray(operations)) throw new TypeError("bulkWrite takes an array of operations");
    const ordered = options.ordered !== false;
    const groups = [];
    const insertedIds = {};
    operations.forEach((operation, index) => {
      const [type, spec] = Object.entries(operation ?? {})[0] ?? [];
      let kind;
      let statement;
      switch (type) {
        case "insertOne":
          kind = "insert";
          statement = mongoWithId(spec.document);
          insertedIds[index] = statement._id;
          break;
        case "updateOne":
        case "updateMany":
          kind = "update";
          statement = mongoUpdateStatement(spec.filter, mongoUpdateDocument(spec.update), type === "updateMany", spec);
          break;
        case "replaceOne":
          kind = "update";
          statement = mongoUpdateStatement(spec.filter, mongoReplacement(spec.replacement), false, spec);
          break;
        case "deleteOne":
        case "deleteMany":
          kind = "delete";
          statement = {
            q: mongoDocument(spec.filter, "a filter"),
            limit: type === "deleteOne" ? 1 : 0,
            ...mongoPick(spec, ["hint", "collation"]),
          };
          break;
        default:
          throw new TypeError(`bulkWrite does not know the operation ${type}`);
      }
      let group = ordered ? groups.at(-1) : groups.find((g) => g.kind === kind);
      if (group === undefined || group.kind !== kind) {
        group = { kind, statements: [], indexes: [] };
        groups.push(group);
      }
      group.statements.push(statement);
      group.indexes.push(index);
    });
    const fields = { insert: "documents", update: "updates", delete: "deletes" };
    const result = {
      acknowledged: true,
      insertedCount: 0,
      matchedCount: 0,
      modifiedCount: 0,
      deletedCount: 0,
      upsertedCount: 0,
      insertedIds,
      upsertedIds: {},
    };
    const all = { writeErrors: [], writeConcernError: null };
    for (const group of groups) {
      const written = await mongoWrite(this, group.kind, fields[group.kind], group.statements, options, ordered);
      if (group.kind === "insert") result.insertedCount += written.n;
      if (group.kind === "delete") result.deletedCount += written.n;
      if (group.kind === "update") {
        result.matchedCount += written.n - written.upserted.length;
        result.modifiedCount += written.nModified;
        result.upsertedCount += written.upserted.length;
        for (const u of written.upserted) result.upsertedIds[group.indexes[u.index]] = u._id;
      }
      for (const e of written.writeErrors) all.writeErrors.push({ ...e, index: group.indexes[e.index] });
      all.writeConcernError ??= written.writeConcernError;
      if (ordered && written.writeErrors.length > 0) break;
    }
    mongoThrowWrite(all, result);
    return result;
  }

  // -- reads --------------------------------------------------------------------

  /// A cursor over the documents matching `filter`. Nothing is sent until it
  /// is read; `sort`, `limit`, `skip`, `project` and `batchSize` chain until
  /// then.
  find(filter = {}, options = {}) {
    mongoDocument(filter, "a filter");
    return new MongoCursor(this.db, this, (o) => ({
      find: this.collectionName,
      filter,
      ...mongoPick(o, [
        "sort", "projection", "hint", "skip", "limit", "batchSize", "singleBatch", "comment",
        "maxTimeMS", "max", "min", "returnKey", "showRecordId", "allowDiskUse", "collation", "let",
      ]),
      ...mongoReadConcern(this.db.client, o),
    }), options);
  }

  async findOne(filter = {}, options = {}) {
    const cursor = this.find(filter, { ...options, limit: 1, singleBatch: true });
    try {
      return await cursor.next();
    } finally {
      await cursor.close();
    }
  }

  aggregate(pipeline = [], options = {}) {
    return new MongoCursor(
      this.db,
      this,
      (o) => mongoAggregateCommand(this.collectionName, pipeline, o, this.db.client),
      options,
    );
  }

  async countDocuments(filter = {}, options = {}) {
    const pipeline = [{ $match: mongoDocument(filter, "a filter") }];
    if (options.skip !== undefined) pipeline.push({ $skip: options.skip });
    if (options.limit !== undefined) pipeline.push({ $limit: options.limit });
    pipeline.push({ $group: { _id: 1, n: { $sum: 1 } } });
    const [row] = await this.aggregate(pipeline, options).toArray();
    return row === undefined ? 0 : Number(row.n);
  }

  async estimatedDocumentCount(options = {}) {
    const reply = await this._run(
      { count: this.collectionName, ...mongoPick(options, ["comment", "maxTimeMS"]), ...mongoReadConcern(this.db.client, options) },
      options,
    );
    return Number(reply.n);
  }

  async distinct(key, filter = {}, options = {}) {
    const reply = await this._run(
      {
        distinct: this.collectionName,
        key: String(key),
        query: mongoDocument(filter, "a filter"),
        ...mongoPick(options, ["collation", "comment", "hint", "maxTimeMS"]),
        ...mongoReadConcern(this.db.client, options),
      },
      options,
    );
    return reply.values;
  }

  findOneAndUpdate(filter, update, options = {}) {
    return this.#findAndModify(filter, { update: mongoUpdateDocument(update) }, options);
  }

  findOneAndReplace(filter, replacement, options = {}) {
    return this.#findAndModify(filter, { update: mongoReplacement(replacement) }, options);
  }

  findOneAndDelete(filter, options = {}) {
    return this.#findAndModify(filter, { remove: true }, options);
  }

  /// The document before the change (`returnDocument: "before"`, the default)
  /// or after it, or `null` when nothing matched.
  async #findAndModify(filter, change, options) {
    const reply = await this._run(
      {
        findAndModify: this.collectionName,
        query: mongoDocument(filter, "a filter"),
        ...change,
        ...(change.remove ? {} : { new: options.returnDocument === "after", upsert: options.upsert === true }),
        ...(options.projection === undefined ? {} : { fields: options.projection }),
        ...mongoPick(options, ["sort", "arrayFilters", "hint", "collation", "comment", "maxTimeMS", "let", "bypassDocumentValidation"]),
        ...mongoWriteConcern(this.db.client, options),
      },
      options,
    );
    if (reply.writeConcernError) mongoThrowWrite({ writeErrors: [], writeConcernError: reply.writeConcernError }, reply.value ?? null);
    return reply.value ?? null;
  }

  // -- indexes and the collection itself ----------------------------------------

  /// Creates an index and returns its name.
  async createIndex(keys, options = {}) {
    const [name] = await this.createIndexes([{ ...options, key: keys }], options);
    return name;
  }

  /// Creates indexes from `{ key, name?, unique?, … }` specs; returns their names.
  async createIndexes(specs, options = {}) {
    const indexes = specs.map(({ key, signal: _s, timeoutMS: _t, ...rest }) => {
      mongoDocument(key, "an index key");
      const name = rest.name ?? Object.entries(key).map(([field, kind]) => `${field}_${kind}`).join("_");
      return { ...rest, key, name };
    });
    await this._run(
      { createIndexes: this.collectionName, indexes, ...mongoPick(options, ["commitQuorum", "comment"]), ...mongoWriteConcern(this.db.client, options) },
      options,
    );
    return indexes.map((index) => index.name);
  }

  async dropIndex(name, options = {}) {
    await this._run({ dropIndexes: this.collectionName, index: name, ...mongoWriteConcern(this.db.client, options) }, options);
  }

  async dropIndexes(options = {}) {
    await this._run({ dropIndexes: this.collectionName, index: "*", ...mongoWriteConcern(this.db.client, options) }, options);
  }

  listIndexes(options = {}) {
    return new MongoCursor(this.db, this, () => ({
      listIndexes: this.collectionName,
      cursor: options.batchSize === undefined ? {} : { batchSize: options.batchSize },
    }), options);
  }

  /// Drops the collection. Since MongoDB 7.0 dropping one that is not there
  /// succeeds too, so this is `true` either way, as in the official driver.
  async drop(options = {}) {
    await this._run({ drop: this.collectionName, ...mongoWriteConcern(this.db.client, options) }, options);
    return true;
  }
}

// -- cursors ----------------------------------------------------------------------

/// The results of a find, an aggregation or a listing, fetched a batch at a
/// time. Read it with `for await`, `toArray()` or `next()`; leaving a `for await`
/// early, `close()`, or `await using` releases it on the server.
class MongoCursor {
  constructor(db, collection, command, options = {}) {
    this._db = db;
    this._collection = collection;
    this._command = command;
    this._options = { ...options };
    this._started = false;
    this._id = 0n;
    this._buffer = [];
    this._at = 0;
    this._getMoreCollection = null;
    this._closed = false;
    // Without a caller's session, a cursor holds an implicit one from its first
    // command until it is exhausted or closed, so every getMore runs in it.
    this._implicit = null;
  }

  get #commandOptions() {
    return this._implicit === null ? this._options : { ...this._options, _cursorSession: this._implicit };
  }

  async #endImplicit() {
    if (this._implicit !== null) {
      const session = this._implicit;
      this._implicit = null;
      await session.endSession();
    }
  }

  #chain(key, value) {
    if (this._started) throw mongoBad(`${key} cannot change a cursor that has started`);
    this._options[key] = value;
    return this;
  }

  sort(sort) {
    return this.#chain("sort", sort);
  }

  limit(n) {
    return this.#chain("limit", n);
  }

  skip(n) {
    return this.#chain("skip", n);
  }

  project(projection) {
    return this.#chain("projection", projection);
  }

  batchSize(n) {
    return this.#chain("batchSize", n);
  }

  /// The next document, or `null` when there are no more.
  async next() {
    while (this._at >= this._buffer.length) {
      if (this._closed) return null;
      if (!this._started) {
        this._started = true;
        const client = this._db.client;
        if (this._options.session === undefined && client._sessionTimeout !== null) {
          this._implicit = new MongoSession(client, {}, true);
        }
        let reply;
        try {
          reply = await this._db.command(this._command(this._options), this.#commandOptions);
        } catch (e) {
          await this.#endImplicit();
          throw e;
        }
        this.#take(reply.cursor, reply.cursor.firstBatch);
        if (this._id === 0n) await this.#endImplicit();
        continue;
      }
      if (this._id === 0n) {
        this._closed = true;
        return null;
      }
      const reply = await this._db.command(
        {
          getMore: this._id,
          collection: this._getMoreCollection,
          ...(this._options.batchSize === undefined ? {} : { batchSize: this._options.batchSize }),
        },
        this.#commandOptions,
      );
      this.#take(reply.cursor, reply.cursor.nextBatch);
      if (this._id === 0n) await this.#endImplicit();
    }
    return this._buffer[this._at++];
  }

  #take(cursor, batch) {
    this._id = typeof cursor.id === "bigint" ? cursor.id : BigInt(cursor.id ?? 0);
    // The namespace is `db.collection`, and a collection may contain dots.
    const ns = String(cursor.ns ?? "");
    this._getMoreCollection = ns.slice(ns.indexOf(".") + 1);
    this._buffer = batch ?? [];
    this._at = 0;
  }

  async toArray() {
    const out = [];
    try {
      for (let doc = await this.next(); doc !== null; doc = await this.next()) out.push(doc);
    } finally {
      await this.close();
    }
    return out;
  }

  /// Releases the cursor on the server if it is still open there.
  async close() {
    if (this._closed && this._id === 0n && this._implicit === null) return;
    this._closed = true;
    const id = this._id;
    this._id = 0n;
    this._buffer = [];
    if (id !== 0n) {
      await this._db
        .command({ killCursors: this._getMoreCollection, cursors: [id] }, this.#commandOptions)
        .catch(() => {});
    }
    await this.#endImplicit();
  }

  async *[Symbol.asyncIterator]() {
    try {
      for (let doc = await this.next(); doc !== null; doc = await this.next()) yield doc;
    } finally {
      await this.close();
    }
  }

  async [Symbol.asyncDispose]() {
    await this.close();
  }
}

/// The built-in MongoDB driver: `mongodb:` URLs (D186). A client pools its
/// connections in the host, so `pool: { max }` sets that pool's size.
const mongodb = defineDriver({
  name: "mongodb",
  schemes: ["mongodb", "mongodb+srv"],
  dialect: MONGO_DIALECT,
  open: openMongo,
  pooled(url, options = {}, poolOptions = {}) {
    return openMongo(url, poolOptions.max === undefined ? options : { ...options, maxPoolSize: poolOptions.max });
  },
});

export { mongodb };

// The module's default export, here because it names every built-in driver
// and this file is appended last.
export default { connect, sql, queryAst, sqlite, postgres, mysql, mongodb, defineDriver, DbError, DbErrorCode };
