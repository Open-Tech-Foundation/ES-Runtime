
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
  // collection(). Transactions need a replica set; a standalone refuses them.
  supports: { queryText: false, queryAst: false, transactions: true },
});

const MONGO_DEFAULT_PORT = 27017;
const MONGO_DEFAULT_POOL = 100;
const MONGO_DEFAULT_CONNECT_TIMEOUT = 30_000;
const MONGO_DEFAULT_SELECTION_TIMEOUT = 30_000;
const MONGO_DEFAULT_HEARTBEAT = 10_000;
const MONGO_DEFAULT_LOCAL_THRESHOLD = 15;
// How soon a server may be checked again while an operation waits for one.
const MONGO_MIN_HEARTBEAT = 500;
const MONGO_MODES = new Map(
  ["primary", "primaryPreferred", "secondary", "secondaryPreferred", "nearest"].map((m) => [m.toLowerCase(), m]),
);
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

// Server errors a read may be retried after (the retryable reads specification).
const MONGO_RETRYABLE_READ_CODES = new Set([262, 11600, 11602, 10107, 13435, 13436, 189, 134, 91, 7, 6, 89, 9001]);

function mongoRetryableWrite(e) {
  return (
    e instanceof DbError &&
    (e.code === DbErrorCode.ConnectionLost || e.server?.errorLabels?.includes("RetryableWriteError") === true)
  );
}

function mongoRetryableRead(e) {
  return e instanceof DbError && (e.code === DbErrorCode.ConnectionLost || MONGO_RETRYABLE_READ_CODES.has(e.backendCode));
}

/// The server's answer to a transaction number it cannot take, with the fix,
/// as the retryable writes specification words it.
function mongoActionableError(e) {
  if (e instanceof DbError && e.backendCode === 20 && /^Transaction numbers/.test(e.message)) {
    const better = dbError(
      "This MongoDB deployment does not support retryable writes. Please add retryWrites=false to your connection string.",
      DbErrorCode.Unsupported,
      { backendCode: 20 },
    );
    better.server = e.server;
    return better;
  }
  return e;
}

async function mongoActionable(send) {
  try {
    return await send();
  } catch (e) {
    throw mongoActionableError(e);
  }
}

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
    "heartbeatFrequencyMS",
    "journal",
    "loadBalanced",
    "localThresholdMS",
    "maxPoolSize",
    "maxStalenessSeconds",
    "readConcernLevel",
    "readPreference",
    "readPreferenceTags",
    "replicaSet",
    "retryReads",
    "retryWrites",
    "serverSelectionTimeoutMS",
    "srvMaxHosts",
    "srvServiceName",
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

/// The settings for a client: the URL first, then `options` over it. A
/// `mongodb+srv://` URL names one host whose seeds `mongoSeedlist` finds.
function parseMongoConnectionString(url, options = {}) {
  const lower = url.toLowerCase();
  const srv = lower.startsWith("mongodb+srv://");
  if (!srv && !lower.startsWith("mongodb://")) {
    throw mongoBad(`${url} is not a mongodb:// or mongodb+srv:// connection string`);
  }
  let rest = url.slice(srv ? "mongodb+srv://".length : "mongodb://".length);
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
    hosts: [],
    user: null,
    password: null,
    database,
    // mongodb+srv:// implies TLS; tls=false in the URL or options turns it off.
    tls: srv,
    srv: srv ? { host: null, serviceName: "mongodb", maxHosts: 0 } : null,
    maxPoolSize: MONGO_DEFAULT_POOL,
    connectTimeoutMS: MONGO_DEFAULT_CONNECT_TIMEOUT,
    serverSelectionTimeoutMS: MONGO_DEFAULT_SELECTION_TIMEOUT,
    heartbeatFrequencyMS: MONGO_DEFAULT_HEARTBEAT,
    localThresholdMS: MONGO_DEFAULT_LOCAL_THRESHOLD,
    replicaSet: null,
    directConnection: false,
    readPreference: { mode: "primary", tags: [], maxStalenessSeconds: null },
    retryWrites: true,
    retryReads: true,
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

  if (srv) {
    // One host name, and no port: the SRV records give the ports.
    if (hostList.includes(",")) throw mongoBad("a mongodb+srv:// URL names exactly one host");
    if (!/^[A-Za-z0-9_-]+(\.[A-Za-z0-9_-]+)*\.?$/.test(hostList)) {
      throw mongoBad(`${hostList} is not a host name for mongodb+srv:// (a port is not allowed)`);
    }
    settings.srv.host = hostList.toLowerCase().replace(/\.$/, "");
  }
  for (const host of srv ? [] : hostList.split(",")) {
    if (host === "") throw mongoBad(`${url} names an empty host`);
    if (/%2f|\.sock$/i.test(host)) throw mongoBad("Unix-socket connections are not supported yet");
    const ipv6 = /^\[([^\]]+)\](?::(\d+))?$/.exec(host);
    const parts = ipv6 ?? /^([^:]+)(?::(\d+))?$/.exec(host);
    if (!parts) throw mongoBad(`${host} is not a host and port`);
    let port = MONGO_DEFAULT_PORT;
    if (parts[2] !== undefined) {
      port = Number(parts[2]);
      if (!(port >= 1 && port <= 65535)) throw mongoBad(`${parts[2]} is not a port`);
    }
    const name = parts[1].toLowerCase();
    settings.hosts.push(ipv6 ? `[${name}]:${port}` : `${name}:${port}`);
  }

  const given = {};
  const tagSets = [];
  for (const pair of query.split("&")) {
    if (pair === "") continue;
    const eq = pair.indexOf("=");
    const key = decodeURIComponent(eq >= 0 ? pair.slice(0, eq) : pair);
    const value = eq >= 0 ? decodeURIComponent(pair.slice(eq + 1)) : "";
    // The one option that may repeat: each is a tag set, tried in order.
    if (key.toLowerCase() === "readpreferencetags") tagSets.push(mongoTagSet(value));
    else given[key] = value;
  }
  if (tagSets.length > 0) given.readPreferenceTags = tagSets;
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
  // What the URL and options named, so a TXT record cannot override them.
  if (srv) settings.srv.named = new Set(Object.keys(given).map((key) => key.toLowerCase()));
  mongoCheckSrv(settings);
  // With SRV, a TXT record may still supply it.
  if (!srv) settings.authSource ??= settings.database ?? "admin";
  if (settings.directConnection && (srv || settings.hosts.length > 1)) {
    throw mongoBad(
      srv ? "directConnection=true cannot be used with mongodb+srv://" : "directConnection=true cannot name more than one host",
    );
  }
  mongoCheckPreference(settings.readPreference);
  return settings;
}

/// The SRV options' rules, applied once after the URL and again after a TXT
/// record has added its options.
function mongoCheckSrv(settings) {
  if (settings.srv === null) {
    if (settings.srvGiven) throw mongoBad("srvServiceName and srvMaxHosts need a mongodb+srv:// URL");
    return;
  }
  if (settings.srv.maxHosts > 0) {
    if (settings.replicaSet !== null) throw mongoBad("srvMaxHosts cannot be used with replicaSet");
    if (settings.loadBalanced) throw mongoBad("srvMaxHosts cannot be used with loadBalanced=true");
  }
}

/// `mongodb+srv://`'s discovery (D188): the seeds from the SRV records — every
/// one checked, in Rust, to be in the URL host's domain — and the TXT record's
/// `authSource`, `replicaSet` and `loadBalanced`, which the URL and options
/// override.
async function mongoSeedlist(settings) {
  const { host, serviceName, maxHosts, named } = settings.srv;
  const found = await ops.mongo_seedlist(host, serviceName, maxHosts);
  if (found.error !== undefined) throw mongoBad(`mongodb+srv://${host}: ${found.error}`);
  settings.hosts = found.hosts;
  for (const [name, value] of found.txt) {
    if (!named.has(name.toLowerCase())) mongoOption(settings, name, value);
  }
  mongoCheckSrv(settings);
  settings.authSource ??= settings.database ?? "admin";
}

/// `dc:ny,rack:1` as `{ dc: "ny", rack: "1" }`; empty is `{}`, any server.
function mongoTagSet(text) {
  const set = {};
  for (const pair of String(text).split(",")) {
    if (pair === "") continue;
    const colon = pair.indexOf(":");
    if (colon <= 0) throw mongoBad(`${text} is not a tag set like dc:ny,rack:1`);
    set[pair.slice(0, colon)] = pair.slice(colon + 1);
  }
  return set;
}

/// A read preference from what a caller passes: a mode name, or
/// `{ mode, tags, maxStalenessSeconds }`.
function mongoPreference(value) {
  if (typeof value === "string") value = { mode: value };
  if (value === null || typeof value !== "object") {
    throw new TypeError("a read preference is a mode name or { mode, tags, maxStalenessSeconds }");
  }
  const mode = MONGO_MODES.get(String(value.mode ?? "primary").toLowerCase());
  if (mode === undefined) throw mongoBad(`${value.mode} is not a read preference mode`);
  const preference = {
    mode,
    tags: value.tags ?? [],
    maxStalenessSeconds: value.maxStalenessSeconds ?? null,
  };
  mongoCheckPreference(preference);
  return preference;
}

function mongoCheckPreference(preference) {
  const { mode, tags, maxStalenessSeconds } = preference;
  if (!Array.isArray(tags)) throw new TypeError("read preference tags are an array of tag sets");
  if (mode === "primary" && tags.some((t) => Object.keys(t).length > 0)) {
    throw mongoBad("tag sets cannot be used with read preference primary");
  }
  if (maxStalenessSeconds !== null && maxStalenessSeconds !== -1) {
    if (!Number.isInteger(maxStalenessSeconds) || maxStalenessSeconds <= 0) {
      throw mongoBad("maxStalenessSeconds must be a positive whole number, or -1 for none");
    }
    if (mode === "primary") throw mongoBad("maxStalenessSeconds cannot be used with read preference primary");
  }
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
      settings.serverSelectionTimeoutMS = mongoCount(name, value);
      return;
    case "heartbeatFrequencyMS": {
      const n = mongoCount(name, value);
      if (n < MONGO_MIN_HEARTBEAT) throw mongoBad(`heartbeatFrequencyMS must be at least ${MONGO_MIN_HEARTBEAT}`);
      settings.heartbeatFrequencyMS = n;
      return;
    }
    case "localThresholdMS":
      settings.localThresholdMS = mongoCount(name, value);
      return;
    case "directConnection":
      settings.directConnection = mongoBoolean(name, value);
      return;
    case "replicaSet":
      settings.replicaSet = String(value);
      return;
    case "loadBalanced":
      if (mongoBoolean(name, value)) {
        throw mongoBad("load-balanced deployments are not supported yet; connect to the replica set");
      }
      settings.loadBalanced = false;
      return;
    case "srvServiceName":
      settings.srvGiven = true;
      if (settings.srv !== null) settings.srv.serviceName = String(value);
      return;
    case "srvMaxHosts":
      settings.srvGiven = true;
      if (settings.srv !== null) settings.srv.maxHosts = mongoCount(name, value);
      return;
    case "readPreference": {
      const mode = MONGO_MODES.get(String(value).toLowerCase());
      if (mode === undefined) throw mongoBad(`${value} is not a read preference mode`);
      settings.readPreference = { ...settings.readPreference, mode };
      return;
    }
    case "readPreferenceTags":
      settings.readPreference = {
        ...settings.readPreference,
        tags: Array.isArray(value) ? value.map((t) => (typeof t === "string" ? mongoTagSet(t) : t)) : [mongoTagSet(value)],
      };
      return;
    case "maxStalenessSeconds": {
      const n = Number(value);
      settings.readPreference = { ...settings.readPreference, maxStalenessSeconds: n === -1 ? null : n };
      return;
    }
    case "retryReads":
    case "retryWrites":
      settings[name] = mongoBoolean(name, value);
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
  if (settings.srv !== null) await mongoSeedlist(settings);
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
      settings.hosts,
      settings.directConnection,
      settings.replicaSet,
      settings.tls,
      ca,
      settings.user,
      password,
      password.normalize("NFKC"),
      settings.authSource,
      settings.authMechanism ?? null,
      settings.appName ?? null,
      settings.maxPoolSize,
      settings.heartbeatFrequencyMS,
      settings.localThresholdMS,
      ticket,
    );
  } finally {
    if (timer !== null) clearTimeout(timer);
  }
  if (result.error !== undefined) throw mongoFailure(result.error);
  // No seed answered at all: say so now, rather than have the first operation
  // wait out serverSelectionTimeoutMS for a deployment that is not there.
  if (!result.topology.servers.some((server) => server.type !== "Unknown")) {
    await ops.mongo_close(result.id).catch(() => {});
    if (timedOut) {
      throw dbError(
        `connecting to ${settings.hosts.join(", ")} took longer than connectTimeoutMS (${settings.connectTimeoutMS} ms)`,
        DbErrorCode.Timeout,
      );
    }
    if (result.topology.servers.length === 0) {
      // Every seed answered, and none belongs: a replica set by another name,
      // or a mix of servers no one deployment has.
      throw dbError(
        settings.replicaSet === null
          ? "the seeds do not form one deployment"
          : `no seed is a member of replica set ${settings.replicaSet}`,
        DbErrorCode.ConnectionLost,
      );
    }
    const why = result.topology.servers.map((server) => `${server.address}: ${server.error ?? "no answer"}`);
    throw dbError(`no server answered: ${why.join("; ")}`, DbErrorCode.ConnectionLost);
  }
  const client = new MongoConnection(result, settings);
  // Opening a pooled connection authenticates, so wrong credentials fail at
  // connect rather than at the first operation.
  try {
    await client._run("admin", { ping: 1 }, { readPreference: "nearest" });
  } catch (e) {
    await client.close().catch(() => {});
    throw e;
  }
  return client;
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
  constructor(client, { causalConsistency = true, defaultTransactionOptions = {} } = {}, implicit = false) {
    this.client = client;
    this.explicit = !implicit;
    this.causalConsistency = implicit ? false : causalConsistency !== false;
    this.clusterTime = null;
    this.operationTime = null;
    this.hasEnded = false;
    this.defaultTransactionOptions = defaultTransactionOptions;
    this._server = null;
    // none, starting, in_progress, committed, committed_empty or aborted, as
    // the transactions specification's state machine names them.
    this._txnState = "none";
    this._txnOptions = null;
  }

  /// Whether a transaction is open on this session.
  get inTransaction() {
    return this._txnState === "starting" || this._txnState === "in_progress";
  }

  /// Starts a transaction. Operations passed this session run in it until
  /// `commitTransaction()` or `abortTransaction()`. Nothing is sent until the
  /// first of them.
  startTransaction(options = {}) {
    if (this.hasEnded) throw mongoBad("the session has ended");
    if (this.inTransaction) throw mongoBad("a transaction is already in progress on this session");
    if (!this.client._retryableWritesSupported()) {
      throw mongoBad("MongoDB transactions need a replica set; this server is a standalone");
    }
    const merged = { ...this.defaultTransactionOptions, ...options };
    if (merged.readPreference !== undefined && mongoPreference(merged.readPreference).mode !== "primary") {
      throw mongoBad("a transaction reads from the primary");
    }
    const writeConcern = merged.writeConcern ?? this.client._settings.writeConcern;
    if (writeConcern?.w === 0) throw mongoBad("a transaction cannot use an unacknowledged write concern (w: 0)");
    this._txnOptions = {
      readConcern: merged.readConcern ?? this.client._settings.readConcern,
      writeConcern,
      maxCommitTimeMS: merged.maxCommitTimeMS,
    };
    this._serverSession().txnNumber += 1n;
    this._txnState = "starting";
  }

  #control(name) {
    const writeConcern = { ...this._txnOptions.writeConcern };
    // Committing again, after a commit whose outcome was unknown, waits for a
    // majority, so the answer means what it says.
    if (name === "commitTransaction" && this._txnState === "committed") {
      writeConcern.w = "majority";
      writeConcern.wtimeout ??= 10_000;
    }
    return {
      [name]: 1,
      txnNumber: this._serverSession().txnNumber,
      autocommit: false,
      ...(Object.keys(writeConcern).length > 0 ? { writeConcern } : {}),
      ...(name === "commitTransaction" && this._txnOptions.maxCommitTimeMS !== undefined
        ? { maxTimeMS: this._txnOptions.maxCommitTimeMS }
        : {}),
    };
  }

  /// Commits the transaction. A commit whose outcome is unknown — its
  /// connection lost, or its write concern not yet satisfied — fails with the
  /// UnknownTransactionCommitResult label, and may be committed again.
  async commitTransaction() {
    switch (this._txnState) {
      case "none":
        throw mongoBad("no transaction has been started");
      case "aborted":
        throw mongoBad("the transaction was aborted, so it cannot be committed");
      case "starting":
      case "committed_empty":
        // Nothing was sent, so there is nothing to commit.
        this._txnState = "committed_empty";
        return;
    }
    const command = this.#control("commitTransaction");
    this._txnState = "committed";
    let doc;
    try {
      doc = await this.client._run("admin", command, { session: this, _txnControl: true });
    } catch (e) {
      throw mongoCommitError(e, false);
    }
    if (doc.writeConcernError !== undefined) {
      throw mongoCommitError(mongoServerError({ ...doc.writeConcernError, errorLabels: doc.errorLabels }), true);
    }
  }

  /// Abandons the transaction. The server's answer is not reported: the
  /// transaction is over either way.
  async abortTransaction() {
    switch (this._txnState) {
      case "none":
        throw mongoBad("no transaction has been started");
      case "committed":
      case "committed_empty":
        throw mongoBad("the transaction was committed, so it cannot be aborted");
      case "aborted":
        throw mongoBad("the transaction was already aborted");
      case "starting":
        this._txnState = "aborted";
        return;
    }
    const command = this.#control("abortTransaction");
    this._txnState = "aborted";
    await this.client._run("admin", command, { session: this, _txnControl: true }).catch(() => {});
  }

  /// Runs `fn` in a transaction and commits it, retrying the whole of it on a
  /// TransientTransactionError and the commit on an
  /// UnknownTransactionCommitResult, for up to 120 seconds with the jittered
  /// backoff the convenient transactions API specifies. Returns what `fn`
  /// returns; `fn` receives this session.
  async withTransaction(fn, options = {}) {
    const started = Date.now();
    const limit = 120_000;
    let attempt = 0;
    let lastError = null;
    for (;;) {
      if (attempt > 0) {
        const backoff = Math.random() * Math.min(5 * 1.5 ** attempt, 500);
        if (Date.now() + backoff - started >= limit) throw lastError;
        await new Promise((resolve) => setTimeout(resolve, backoff));
      }
      this.startTransaction(options);
      attempt++;
      let result;
      try {
        result = await fn(this);
      } catch (e) {
        lastError = e;
        if (this.inTransaction) await this.abortTransaction().catch(() => {});
        if (mongoHasLabel(e, "TransientTransactionError") && Date.now() - started < limit) continue;
        throw e;
      }
      // The callback ended the transaction itself.
      if (!this.inTransaction) return result;
      for (;;) {
        try {
          await this.commitTransaction();
          return result;
        } catch (e) {
          lastError = e;
          if (e.backendCode !== 50 && mongoHasLabel(e, "UnknownTransactionCommitResult")) {
            if (Date.now() - started >= limit) throw e;
            continue;
          }
          if (mongoHasLabel(e, "TransientTransactionError")) break;
          throw e;
        }
      }
    }
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
    // A transaction still open when its session ends is abandoned.
    if (this.inTransaction) await this.abortTransaction().catch(() => {});
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

// Commands that write, and so go to a primary (or a mongos).
const MONGO_WRITES = new Set([
  "insert", "update", "delete", "findAndModify", "create", "drop", "dropDatabase", "createIndexes",
  "dropIndexes", "collMod", "renameCollection", "commitTransaction", "abortTransaction",
]);

/// What `client.transaction(fn)` hands `fn`: the client itself, as a
/// connection is in every other runtime:db driver, except that its databases
/// and commands run in the transaction's session, which is `tx.session`.
function mongoTransactionView(client, session) {
  const view = Object.create(client);
  Object.defineProperties(view, {
    session: { value: session },
    db: { value: (name = client._settings.database ?? "test") => new MongoDb(client, name, session) },
    command: { value: (command, options = {}) => view.db(options.db ?? "admin").command(command, options) },
  });
  return view;
}

function mongoHasLabel(e, label) {
  return e?.server?.errorLabels?.includes(label) === true;
}

function mongoAddLabel(e, label) {
  if (!(e instanceof DbError)) return e;
  e.server ??= { code: e.backendCode ?? 0, codeName: "", errorLabels: [] };
  e.server.errorLabels ??= [];
  if (!e.server.errorLabels.includes(label)) e.server.errorLabels.push(label);
  return e;
}

/// A commit whose outcome is unknown gets the UnknownTransactionCommitResult
/// label: a lost connection, no server to send it to, a retryable error, a
/// time limit, or a write concern not satisfied (other than one that can never
/// be: CannotSatisfyWriteConcern and UnknownReplWriteConcern).
function mongoCommitError(e, writeConcern) {
  const unknown =
    (e instanceof DbError && e.code === DbErrorCode.ConnectionLost) ||
    e?.mongoSelection === true ||
    mongoHasLabel(e, "RetryableWriteError") ||
    e?.backendCode === 50 ||
    (writeConcern && e?.backendCode !== 100 && e?.backendCode !== 79);
  if (unknown) {
    mongoAddLabel(e, "UnknownTransactionCommitResult");
    // Unknown is not transient: retrying the whole transaction could apply it twice.
    if (e.server?.errorLabels) e.server.errorLabels = e.server.errorLabels.filter((l) => l !== "TransientTransactionError");
  }
  return e;
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
    this._sessions = new MongoServerSessionPool();
    this._clusterTime = null;
    this._checking = null;
    this._applyTopology(result.topology);
    // Servers are checked every heartbeat from an unreferenced timer: a program
    // that has finished its work exits without closing the client.
    this._heartbeat = setInterval(() => this._check(), settings.heartbeatFrequencyMS);
    globalThis.__timer_ref?.(this._heartbeat, false);
  }

  _applyTopology(topology) {
    this._topology = topology;
    this._maxMessageSize = topology.maxMessageSizeBytes;
    this._maxBsonObjectSize = topology.maxBsonObjectSize;
    this._maxWriteBatchSize = topology.maxWriteBatchSize;
    // The smallest any data-bearing server reports; null when one has no
    // sessions, and then no session is sent.
    this._sessionTimeout = topology.logicalSessionTimeoutMinutes;
  }

  /// Checks every server now. One check at a time: a caller arriving during
  /// one waits for it.
  _check() {
    this._checking ??= ops
      .mongo_check(this._id)
      .then((topology) => {
        if (topology.error === undefined) this._applyTopology(topology);
      })
      .catch(() => {})
      .finally(() => {
        this._checking = null;
      });
    return this._checking;
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
    return new MongoDb(this, name, null);
  }

  /// Runs a command against `options.db` (default `admin`) and returns its reply.
  command(command, options = {}) {
    return this.db(options.db ?? "admin").command(command, options);
  }

  async ping(options = {}) {
    await this.command({ ping: 1 }, options);
  }

  /// Runs `fn` in a transaction on a new session and commits it, retrying as
  /// `withTransaction` does. `fn` receives this client seen through the
  /// transaction: its databases and collections run in it without being
  /// passed `{ session }`, and the session is `tx.session`. Returns what `fn`
  /// returns.
  async transaction(fn, options = {}) {
    this._open();
    const session = this.startSession();
    try {
      return await session.withTransaction(() => fn(mongoTransactionView(this, session)), options);
    } finally {
      await session.endSession();
    }
  }

  async _close() {
    clearInterval(this._heartbeat);
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
    const inTransaction = session !== null && this.#transactional(session, name, body, options);
    if (inTransaction) {
      // Inside a transaction an operation is neither retried nor routed on its
      // own: the transaction is retried as a whole, on the primary.
      options = {
        ...options,
        _retryableWrite: false,
        _retryableRead: false,
        _inTransaction: true,
        _inheritPreference: false,
        readPreference: undefined,
      };
    }
    try {
      return await this.#retrying(dbName, name, body, session, options, timeoutMS, signal, sequence, unacknowledged);
    } catch (e) {
      // A network error inside a transaction leaves it to be retried whole.
      if (inTransaction && e instanceof DbError && e.code === DbErrorCode.ConnectionLost) {
        mongoAddLabel(e, "TransientTransactionError");
      }
      throw e;
    } finally {
      if (owned) await session.endSession();
    }
  }

  /// Adds a transaction's fields to a command run in one, and says whether it
  /// is. The first command starts the transaction and carries its read
  /// concern; every command carries the transaction number, and none its own
  /// write concern.
  #transactional(session, name, body, options) {
    if (name === "commitTransaction" || name === "abortTransaction") return false;
    // An operation after a transaction ended runs outside one.
    if (!session.inTransaction) {
      if (session._txnState !== "none") session._txnState = "none";
      return false;
    }
    if (options.writeConcern !== undefined) {
      throw mongoBad("an operation in a transaction cannot set a write concern; set it on the transaction");
    }
    if (options.readPreference !== undefined && mongoPreference(options.readPreference).mode !== "primary") {
      throw mongoBad("an operation in a transaction reads from the primary");
    }
    delete body.writeConcern;
    delete body.readConcern;
    body.txnNumber = session._serverSession().txnNumber;
    body.autocommit = false;
    if (session._txnState === "starting") {
      body.startTransaction = true;
      const readConcern = { ...session._txnOptions.readConcern };
      if (session.causalConsistency && session.operationTime !== null) {
        readConcern.afterClusterTime = session.operationTime;
      }
      if (Object.keys(readConcern).length > 0) body.readConcern = readConcern;
      session._txnState = "in_progress";
    }
    return true;
  }

  /// Whether the deployment takes retryable writes: it has sessions, and is not
  /// a standalone.
  _retryableWritesSupported() {
    const topology = this._topology;
    if (topology.logicalSessionTimeoutMinutes == null) return false;
    return !(topology.type === "Single" && topology.servers[0]?.type === "Standalone");
  }

  /// Sends a command, and once more if the retryable writes or retryable reads
  /// specification allows. A retried write carries the same lsid and
  /// txnNumber, so the server applies it at most once.
  async #retrying(dbName, name, body, session, options, timeoutMS, signal, sequence, unacknowledged) {
    const send = () => this.#send(dbName, name, body, session, options, timeoutMS, signal, sequence);
    // commitTransaction and abortTransaction are retryable writes whatever
    // retryWrites says, and keep the transaction's number.
    const control = options._txnControl === true;
    const write =
      control ||
      (options._retryableWrite === true &&
        session !== null &&
        !unacknowledged &&
        this._settings.retryWrites &&
        this._retryableWritesSupported());
    const read = options._retryableRead === true && this._settings.retryReads;
    if (write && !control) {
      const server = session._serverSession();
      server.txnNumber += 1n;
      body.txnNumber = server.txnNumber;
    }
    if (!write && !read) return mongoActionable(send);

    let first;
    try {
      const doc = await send();
      // A write concern error can be labelled retryable too.
      if (!(write && doc.errorLabels?.includes("RetryableWriteError"))) return doc;
      first = { doc };
    } catch (e) {
      if (!(write ? mongoRetryableWrite(e) : mongoRetryableRead(e))) throw mongoActionableError(e);
      first = { error: e };
    }
    const previous = () => {
      if (first.doc !== undefined) return first.doc;
      throw first.error;
    };
    if (control && name === "commitTransaction") {
      body.writeConcern = { ...body.writeConcern, w: "majority", wtimeout: body.writeConcern?.wtimeout ?? 10_000 };
    }
    try {
      return await send();
    } catch (e) {
      // No server for the retry, or a retry the server says wrote nothing: the
      // first attempt's outcome is the one to report.
      if (e?.mongoSelection === true || e?.server?.errorLabels?.includes("NoWritesPerformed")) return previous();
      throw mongoActionableError(e);
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
        MONGO_CAUSAL_READS.has(name) &&
        options._inTransaction !== true
      ) {
        body.readConcern = { ...body.readConcern, afterClusterTime: session.operationTime };
      }
    }
    const preference =
      options.readPreference !== undefined
        ? mongoPreference(options.readPreference)
        : options._inheritPreference
          ? this._settings.readPreference
          : { mode: "primary", tags: [], maxStalenessSeconds: null };
    const kind = options._kind ?? (MONGO_WRITES.has(name) ? "write" : "read");
    const clusterTime = mongoTimestampAfter(session?.clusterTime?.clusterTime, this._clusterTime?.clusterTime)
      ? session.clusterTime
      : this._clusterTime;
    if (clusterTime !== null) body.$clusterTime = clusterTime;
    body.$db = dbName;
    const bytes = MongoBSON.encode(body);

    const ticket = mongoTicket++;
    let stopped = null;
    let wake = null;
    const stop = (reason) => {
      if (stopped !== null) return;
      stopped = reason;
      wake?.();
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
    // Server selection: when no server can take the command, check the
    // deployment again and retry until serverSelectionTimeoutMS, as the server
    // selection specification has a client wait.
    const selectionTimeout = this._settings.serverSelectionTimeoutMS;
    const deadline = Date.now() + selectionTimeout;
    let ran;
    try {
      for (;;) {
        ran = await ops.mongo_command(
          this._id,
          bytes,
          ticket,
          sequence?.name ?? null,
          sequence?.documents ?? null,
          preference.mode,
          preference.tags.length > 0 ? preference.tags : null,
          preference.maxStalenessSeconds,
          kind,
          options._pin ?? null,
        );
        if (ran.error?.kind !== "selection" || stopped !== null || Date.now() >= deadline) break;
        await this._check();
        if (stopped !== null) break;
        await new Promise((resolve) => {
          wake = resolve;
          setTimeout(resolve, Math.max(0, Math.min(MONGO_MIN_HEARTBEAT, deadline - Date.now())));
        });
        wake = null;
      }
    } finally {
      if (timer !== null) clearTimeout(timer);
      signal?.removeEventListener("abort", onAbort);
    }
    // A command stopped mid-flight or lost with its connection may have left
    // the server session in an unknown state, so it is not reused.
    if (server !== null && (stopped !== null || ran.error?.kind === "lost")) {
      server.dirty = true;
    }
    // Stopped while it ran is stopped, however the command ended.
    if (stopped !== null) throw stopped;
    if (ran.error?.kind === "selection") {
      throw Object.assign(
        dbError(
          `no server could take the operation within serverSelectionTimeoutMS (${selectionTimeout} ms): ${ran.error.message}`,
          DbErrorCode.Timeout,
        ),
        { mongoSelection: true },
      );
    }
    if (ran.error !== undefined) throw mongoFailure(ran.error);
    // A state-change error asks for the deployment to be checked now.
    if (ran.recheck) this._check();
    if (options._served !== undefined) options._served.server = ran.server;
    const doc = MongoBSON.decode(ran.reply);
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
  constructor(client, name, session) {
    this.client = client;
    this.databaseName = mongoName("database", name);
    // Set when reached through a transaction's view: every operation runs in it.
    this._session = session;
  }

  _withSession(options) {
    return this._session === null || options.session !== undefined ? options : { ...options, session: this._session };
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
    return this.client._run(this.databaseName, command, this._withSession(options));
  }

  /// A database-level aggregation (`$currentOp`, `$documents`, …).
  aggregate(pipeline = [], options = {}) {
    return new MongoCursor(
      this,
      null,
      () => mongoAggregateCommand(1, pipeline, options, this.client),
      mongoReadOptions(pipeline, options),
    );
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
      { ...options, _inheritPreference: true },
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

/// Options for a cursor's commands: a read follows the client's read
/// preference unless the call gives its own, and an aggregation ending in
/// `$out` or `$merge` is a write.
function mongoReadOptions(pipeline, options) {
  const last = pipeline?.at(-1);
  const writes = last !== undefined && last !== null && ("$out" in last || "$merge" in last);
  return writes ? { ...options, _kind: "write" } : { ...options, _inheritPreference: true };
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
    // A command is a retryable write when no statement in it can touch more
    // than one document.
    const batch = statements.slice(start, end);
    const retryable =
      kind === "insert" ||
      (kind === "update" && batch.every((st) => st.multi !== true)) ||
      (kind === "delete" && batch.every((st) => st.limit === 1));
    const reply = await collection._run(
      { [kind]: collection.collectionName, ...extra },
      { ...options, _retryableWrite: retryable },
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
    return this.db.client._run(this.db.databaseName, command, this.db._withSession(options), sequence);
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
    }), { ...options, _inheritPreference: true });
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
      mongoReadOptions(pipeline, options),
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
      { ...options, _inheritPreference: true, _retryableRead: true },
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
      { ...options, _inheritPreference: true, _retryableRead: true },
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
      { ...options, _retryableWrite: true },
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
    }), { ...options, _inheritPreference: true });
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
    this._server = null;
  }

  /// Options for the cursor's commands: its session, and after the first,
  /// the server it lives on, where every getMore and killCursors must go.
  get #commandOptions() {
    const options = { ...this._options };
    if (this._implicit !== null) options._cursorSession = this._implicit;
    if (this._server !== null) options._pin = this._server;
    return options;
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
        if (this._options.session === undefined && this._db._session === null && client._sessionTimeout !== null) {
          this._implicit = new MongoSession(client, {}, true);
        }
        let reply;
        try {
          const served = {};
          // The first command of a read cursor is a retryable read; a getMore
          // never is.
          reply = await this._db.command(this._command(this._options), {
            ...this.#commandOptions,
            _served: served,
            _retryableRead: this._options._kind !== "write",
          });
          this._server = served.server ?? null;
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
