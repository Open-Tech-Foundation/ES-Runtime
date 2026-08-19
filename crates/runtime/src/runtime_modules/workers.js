// runtime:workers — durable workers (DECISIONS D80).
//
// A durable worker is addressable rather than spawned: you name one, and the
// runtime materializes it on demand, runs one call at a time against it, and
// keeps its state in SQLite so the next process to name it finds it where it
// was left. `Worker` — the HTML one — is a global and stays exactly that. A
// `DurableWorker` is **not** a `Worker`: it is a unit of state and single-
// threaded execution that will *run on* workers once it is sharded.
//
//     import { DurableWorker } from "runtime:workers";
//
//     export class Counter extends DurableWorker {
//       async add(n) {
//         const total = (this.state.get("total") ?? 0) + n;
//         this.state.set("total", total);
//         return total;                       // held until that write commits
//       }
//     }
//
//     await Counter.get("hits").add(1);
//
// Three properties are the whole design, and each is a rule rather than an
// aspiration:
//
//   * **One call at a time, per worker.** Calls queue in that worker's mailbox.
//     There is no lock to take and no race to lose, which is the reason to
//     address state by worker rather than by row.
//   * **Reads are synchronous, writes are gated.** A worker's key/value state is
//     resident in its heap, so `get` is a map lookup. `set` returns a promise
//     that resolves when the write is durable, and **no call's result is
//     delivered before the writes it made have committed** — so a crash cannot
//     leave a caller believing something the disk never heard about. That gate
//     is why the writes may be coalesced at all.
//   * **The runtime owns the schema.** No DDL, no SQL, no migration script.
//
// What is deliberately not here yet, and arrives in its own phase rather than
// as a flag that does nothing: shards (every worker runs in the host agent
// today), collections, and alarms.
//
// **Capabilities.** This module adds none. It reads and writes files under its
// own directory, so it needs `--allow-read`/`--allow-write` exactly as
// `runtime:db` does, and nothing else — the durability is SQLite's and the
// authority is the filesystem's.

import { connect, sqlite, sql } from "runtime:db";
import { mkdir, remove } from "runtime:fs";
import { hash } from "runtime:hashing";

// Captured at load: these are how a value becomes bytes and back, and a program
// that later removes the globals must not silently change what its state means.
const serialize = globalThis.__structuredSerialize;
const deserialize = globalThis.__structuredDeserialize;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

const DurableErrorCode = Object.freeze({
  /// The mailbox is full: this worker has more calls waiting than `mailbox`
  /// allows. Distinct from slow — it is the queue refusing to grow, which is
  /// the only honest answer when the thing being queued for is already behind.
  Busy: "ERR_DURABLE_BUSY",
  /// Another process has this directory open. A durable directory belongs to
  /// one process at a time — two of them materializing the same worker would
  /// each believe they were the only one — and the engine enforces it with an
  /// exclusive lock on the file, which the OS releases when that process ends.
  Locked: "ERR_DURABLE_LOCKED",
  /// A value, or the whole of a worker's key/value state, is over the limit.
  /// The state is resident in memory, so the ceiling is real rather than
  /// advisory: what does not fit belongs in a database of its own.
  StateTooLarge: "ERR_DURABLE_STATE_TOO_LARGE",
  /// Stored bytes this build cannot read — written by a newer runtime whose
  /// serialization format this one does not know. Refused rather than guessed.
  StateFormat: "ERR_DURABLE_STATE_FORMAT",
  /// Two ids hashed to one file. Astronomically unlikely and checked anyway,
  /// because the alternative is two workers quietly sharing one state.
  IdCollision: "ERR_DURABLE_ID_COLLISION",
  /// The runtime is shutting down and will not start new work.
  Shutdown: "ERR_DURABLE_SHUTDOWN",
  /// `configure()` after the first worker was materialized. The settings decide
  /// where state lives, so changing them mid-flight would split it.
  Configured: "ERR_DURABLE_CONFIGURED",
});

class DurableError extends Error {
  constructor(message, code, options) {
    super(message, options);
    this.name = "DurableError";
    this.code = code;
  }
}

const fail = (code, message, options) => {
  throw new DurableError(message, code, options);
};

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

// Every default is a number somebody will want to change, so each is named
// rather than buried at its use site.
const defaults = {
  // Where state lives. Relative to the working directory, which is also the
  // jail (D79), so this is inside the deployment by construction.
  dir: "./.durable",
  // How long a worker may sit idle before it is closed. Eviction is checked
  // when work arrives rather than on a timer: a timer would be a reason the
  // process could never exit, and a script that used a durable worker once
  // would then hang for exactly this long.
  evictAfter: 30_000,
  // How many workers may be live — and therefore how many database handles are
  // open — at once. Past it, the least recently used are closed.
  maxLive: 128,
  // How many calls may wait on one worker before further ones are refused.
  mailbox: 1024,
  // The ceiling on one worker's key/value state, and on a single value. State
  // is resident, so these bound memory as much as they bound the file.
  stateLimit: 1024 * 1024,
  valueLimit: 128 * 1024,
};

let config = { ...defaults };
let started = false;

const positive = (value, name) => {
  if (typeof value !== "number" || !Number.isFinite(value) || value <= 0) {
    throw new TypeError(`${name} must be a positive number`);
  }
  return value;
};

/**
 * Settings for every durable worker in this process. Optional — with no call
 * at all the defaults apply, which is the point: the first one costs no setup.
 *
 * It must come before the first worker is materialized. The settings decide
 * where state lives and how much of it is held, so changing them with workers
 * already open would split a worker's state across two answers.
 */
function configure(options = {}) {
  if (started) {
    fail(
      DurableErrorCode.Configured,
      "configure() must be called before the first durable worker is used",
    );
  }
  if (options === null || typeof options !== "object") {
    throw new TypeError("configure(options): options must be an object");
  }
  const next = { ...config };
  for (const [key, value] of Object.entries(options)) {
    if (!(key in defaults)) {
      throw new TypeError(
        `unknown option "${key}" — expected one of: ${Object.keys(defaults).join(", ")}`,
      );
    }
    next[key] = key === "dir" ? String(value) : positive(value, key);
  }
  if (next.valueLimit > next.stateLimit) {
    throw new TypeError("valueLimit cannot exceed stateLimit");
  }
  config = next;
  return { ...config };
}

// ---------------------------------------------------------------------------
// The codec
// ---------------------------------------------------------------------------

// 1 is the structured-clone serialization the engine already performs for
// `postMessage` and `structuredClone`: it keeps Date, Map, Set, TypedArray,
// BigInt, Error and cycles, all of which JSON and MessagePack lose. The tag is
// stored beside every value so a format this build cannot read is refused by
// name instead of being handed to a deserializer that will misread it.
const CODEC_STRUCTURED_CLONE = 1;

function encode(value, key) {
  if (typeof serialize !== "function") {
    fail(DurableErrorCode.StateFormat, "this build cannot serialize durable state");
  }
  let bytes;
  try {
    bytes = serialize(value);
  } catch (e) {
    throw new TypeError(
      `state.set(${JSON.stringify(key)}): the value cannot be stored — ${e?.message ?? e}`,
      { cause: e },
    );
  }
  if (bytes.byteLength > config.valueLimit) {
    fail(
      DurableErrorCode.StateTooLarge,
      `state.set(${JSON.stringify(key)}): ${bytes.byteLength} bytes is over the ` +
        `${config.valueLimit}-byte limit for one value`,
    );
  }
  return bytes;
}

function decode(bytes, codec, key) {
  if (codec !== CODEC_STRUCTURED_CLONE) {
    fail(
      DurableErrorCode.StateFormat,
      `state ${JSON.stringify(key)} was written in format ${codec}, which this build ` +
        "cannot read — it was written by a newer runtime",
    );
  }
  try {
    return deserialize(bytes);
  } catch (e) {
    fail(
      DurableErrorCode.StateFormat,
      `state ${JSON.stringify(key)} could not be read back: ${e?.message ?? e}`,
      { cause: e },
    );
  }
}

// ---------------------------------------------------------------------------
// SQLite plumbing
// ---------------------------------------------------------------------------

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

// The engine's `busy_timeout` pragma is a no-op, so a second writer is told
// `ERR_DB_BUSY` immediately rather than waited for. Every write goes through
// here: exponential backoff with jitter, because two processes backing off in
// lockstep are two processes colliding again.
async function retryBusy(run, attempts = 12) {
  for (let attempt = 0; ; attempt++) {
    try {
      return await run();
    } catch (e) {
      if (e?.code !== "ERR_DB_BUSY" || attempt >= attempts) throw e;
      await sleep(Math.min(2 ** attempt, 64) * (0.5 + Math.random()));
    }
  }
}

const open = (path) => connect(`sqlite:${path}`, { driver: sqlite });

async function one(db, query, params) {
  const row = await (await db.query(query, params)).first();
  return row ? row.toObject() : null;
}

// A worker's file name is a hash of its id, not the id itself. An id is a
// string a program chose — it may hold slashes, it may be 400 characters, and
// on macOS and Windows "Cart" and "cart" are the same file. A hash is none of
// those things. The id itself is recorded in the file and in the registry, and
// checked on open, so the one-in-2^128 collision is an error rather than two
// workers quietly sharing a state.
const fileKey = (id) => hash("blake3", id, "hex").slice(0, 32);

// ---------------------------------------------------------------------------
// The registry: the catalog of what exists, and the lock on the directory
// ---------------------------------------------------------------------------

const REGISTRY_SCHEMA = [
  `CREATE TABLE IF NOT EXISTS worker (
     class       TEXT NOT NULL,
     id          TEXT NOT NULL,
     file        TEXT NOT NULL,
     created_at  INTEGER NOT NULL,
     last_active INTEGER NOT NULL,
     bytes       INTEGER NOT NULL DEFAULT 0,
     PRIMARY KEY (class, id)
   )`,
];

let registry = null; // { db, dir }

// Every statement on the catalog goes through one queue, because a connection
// is one conversation and the catalog is the one connection that many callers
// share: twelve workers materializing at once would otherwise put twelve
// statements on it at the same time. The embedded engine does not refuse that —
// it panics from its WAL ("end_write_tx called while write lock not held"), so
// the discipline has to be here. Each worker's own database needs none of this:
// it has exactly one writer, its own flush.
let catalogQueue = Promise.resolve();

function onCatalog(run) {
  const next = catalogQueue.then(run, run);
  catalogQueue = next.then(
    () => {},
    () => {},
  );
  return next;
}

async function registryDb() {
  if (registry) return registry.db;
  started = true;
  const dir = config.dir.replace(/\/+$/, "");
  await mkdir(dir, { recursive: true });
  const db = await openOwned(`${dir}/_registry.db`, dir);
  for (const ddl of REGISTRY_SCHEMA) await retryBusy(() => onCatalog(() => db.execute(ddl)));
  registry = { db, dir };
  return db;
}

// A durable directory belongs to one process, and nothing here has to arrange
// that: the engine takes an exclusive lock on a database file for as long as it
// is open, and the OS drops it when the process ends. So the guarantee is real
// rather than advisory, and it needs no heartbeat, leaves nothing stale behind
// when a process is killed, and cannot be lost while it is held.
//
// What is missing is the sentence explaining it, since what surfaces otherwise
// is the engine's `Locking error` on a file the caller never named. There is no
// distinct code to match on — the classification does not reach that far — so
// the message is matched, deliberately and narrowly: an unrecognized failure is
// re-thrown untouched rather than blamed on a lock.
async function openOwned(path, dir) {
  try {
    return await open(path);
  } catch (e) {
    if (!/lock/i.test(e?.message ?? "")) throw e;
    fail(
      DurableErrorCode.Locked,
      `another process has the durable workers in ${dir} open — a directory belongs ` +
        "to one process at a time, and this one is refused until that process exits",
      { cause: e },
    );
  }
}

// ---------------------------------------------------------------------------
// A worker's state
// ---------------------------------------------------------------------------

const UPSERT_ONE = `INSERT INTO _state (key, value, codec) VALUES (?, ?, ?)
   ON CONFLICT(key) DO UPDATE SET value = excluded.value, codec = excluded.codec`;

const WORKER_SCHEMA = [
  `CREATE TABLE IF NOT EXISTS _state (
     key   TEXT PRIMARY KEY,
     value BLOB NOT NULL,
     codec INTEGER NOT NULL
   )`,
  `CREATE TABLE IF NOT EXISTS _meta (key TEXT PRIMARY KEY, value TEXT NOT NULL)`,
];

// Whether two byte strings are the same. Short-circuits on length, which is
// what almost every different value differs in.
function same(a, b) {
  if (a === undefined || a.byteLength !== b.byteLength) return false;
  for (let i = 0; i < a.byteLength; i++) if (a[i] !== b[i]) return false;
  return true;
}

// Marks `promise` as having a rejection handler, and returns it unchanged. The
// rejection still reaches anyone who awaits it.
function handled(promise) {
  promise.catch(() => {});
  return promise;
}

/**
 * The key/value state of one durable worker: resident in this process, written
 * behind the call that changed it, and never handed to a caller before it is on
 * disk (see the gate in `LiveWorker.call`).
 */
class State {
  #db;
  #values = new Map(); // key -> { value, bytes }
  #dirty = new Set();
  #flushing = null;
  #queued = null;
  #bytes = 0;
  #closed = false;

  constructor(db, rows) {
    this.#db = db;
    for (const row of rows) {
      const { key, value, codec } = row;
      this.#values.set(key, {
        value: decode(value, codec, key),
        bytes: value.byteLength,
        encoded: value,
      });
      this.#bytes += value.byteLength;
    }
  }

  #open() {
    if (this.#closed) {
      fail(DurableErrorCode.Shutdown, "this durable worker has been evicted");
    }
  }

  #key(key) {
    if (typeof key !== "string") throw new TypeError("a state key must be a string");
    if (key.length === 0) throw new TypeError("a state key must not be empty");
    return key;
  }

  /** The value stored under `key`, or `undefined`. Synchronous: the state is
   * already here. Mutating what comes back changes nothing on disk — a value is
   * stored by `set`, not by being touched. */
  get(key) {
    this.#open();
    return this.#values.get(this.#key(key))?.value;
  }

  has(key) {
    this.#open();
    return this.#values.has(this.#key(key));
  }

  /** Stores `value`, and resolves once it is durable. The value is visible to
   * `get` immediately; the promise is what "written down" means. */
  set(key, value) {
    this.#open();
    this.#write(this.#key(key), value);
    return this.#schedule();
  }

  /** Several keys in one commit — which is also one transaction and one
   * crossing, so a batch costs about what a single write costs. */
  setMany(entries) {
    this.#open();
    const pairs = entries instanceof Map ? [...entries] : Object.entries(entries ?? {});
    for (const [key, value] of pairs) this.#write(this.#key(key), value);
    return this.#schedule();
  }

  delete(key) {
    this.#open();
    const k = this.#key(key);
    const held = this.#values.get(k);
    if (held) this.#bytes -= held.bytes;
    this.#values.delete(k);
    this.#dirty.add(k);
    return this.#schedule();
  }

  deleteMany(keys) {
    this.#open();
    for (const key of keys) {
      const k = this.#key(key);
      const held = this.#values.get(k);
      if (held) this.#bytes -= held.bytes;
      this.#values.delete(k);
      this.#dirty.add(k);
    }
    return this.#schedule();
  }

  clear() {
    this.#open();
    return this.deleteMany([...this.#values.keys()]);
  }

  getMany(keys) {
    this.#open();
    const out = new Map();
    for (const key of keys) out.set(key, this.#values.get(this.#key(key))?.value);
    return out;
  }

  /** The keys, in sorted order, optionally narrowed. Synchronous, for the same
   * reason `get` is. */
  keys(options = {}) {
    this.#open();
    const { prefix, start, end, limit, reverse = false } = options;
    let keys = [...this.#values.keys()].sort();
    if (prefix !== undefined) keys = keys.filter((k) => k.startsWith(prefix));
    if (start !== undefined) keys = keys.filter((k) => k >= start);
    if (end !== undefined) keys = keys.filter((k) => k < end);
    if (reverse) keys.reverse();
    if (limit !== undefined) keys = keys.slice(0, positive(limit, "limit"));
    return keys;
  }

  /** `[key, value]` pairs, narrowed the same way `keys` is. */
  list(options = {}) {
    return this.keys(options).map((key) => [key, this.#values.get(key).value]);
  }

  /** How many keys are stored. */
  get size() {
    return this.#values.size;
  }

  /** How many bytes they take, which is what the limit is measured against. */
  get bytes() {
    return this.#bytes;
  }

  /** Waits for every write made so far to be durable. The gate does this for a
   * call's result; call it yourself before a side effect that leaves the
   * process — a `fetch`, a message — since only the result is gated. */
  async sync() {
    while (this.#flushing || this.#dirty.size > 0) {
      await (this.#flushing ?? this.#schedule());
    }
  }

  #write(key, value) {
    const bytes = encode(value, key);
    const held = this.#values.get(key);
    // A `set` that stores what is already stored is not a write. It is worth
    // checking because the storing is the expensive half: a commit that changes
    // a page costs several milliseconds against one that changes none, and
    // "read it, put it back" is what a handler written against a resident state
    // does all day. The comparison is over the encoded bytes, so it is exact
    // rather than a guess about object identity.
    if (held && same(held.encoded, bytes)) {
      held.value = value;
      return;
    }
    const next = this.#bytes - (held?.bytes ?? 0) + bytes.byteLength;
    if (next > config.stateLimit) {
      fail(
        DurableErrorCode.StateTooLarge,
        `this worker's state would be ${next} bytes, over the ${config.stateLimit}-byte ` +
          "limit — state this size belongs in a database of its own",
      );
    }
    this.#bytes = next;
    this.#values.set(key, { value, bytes: bytes.byteLength, encoded: bytes });
    this.#dirty.add(key);
  }

  // One flush is in flight at a time. Writes that arrive while it runs are
  // committed by the next one, and every caller waiting on this generation is
  // resolved by the commit that carried it.
  //
  // `handled` is not decoration. `set()` returns this promise and is routinely
  // *not* awaited — the gate at the end of the call is what a program relies on
  // — so a failed commit would otherwise be an unhandled rejection, which in
  // this runtime ends the process. Attaching a handler here marks the promise
  // handled without consuming it: a caller that does await `set()` still sees
  // the failure, and so does the gate.
  #schedule() {
    if (this.#flushing) {
      this.#queued ??= handled(
        this.#flushing.then(
          () => this.#flush(),
          () => this.#flush(),
        ),
      );
      return this.#queued;
    }
    this.#flushing = handled(Promise.resolve().then(() => this.#flush()));
    return this.#flushing;
  }

  async #flush() {
    this.#queued = null;
    const keys = [...this.#dirty];
    this.#dirty.clear();
    if (keys.length === 0 || this.#closed) {
      this.#flushing = null;
      return;
    }
    const puts = [];
    const gone = [];
    for (const key of keys) {
      const held = this.#values.get(key);
      if (held) puts.push([key, held.encoded, CODEC_STRUCTURED_CLONE]);
      else gone.push([key]);
    }
    try {
      await retryBusy(() => this.#commit(puts, gone));
    } catch (e) {
      // The keys go back on the dirty set: a failed commit must not be a write
      // that quietly never happens, and the next flush retries it.
      for (const key of keys) this.#dirty.add(key);
      this.#flushing = null;
      throw e;
    }
    this.#flushing = null;
    if (this.#dirty.size > 0) this.#schedule();
  }

  // One changed key is what a call usually leaves behind, and a statement on
  // its own is already atomic — so the common case is one op crossing and one
  // implicit transaction rather than a BEGIN, a statement and a COMMIT, which
  // is three.
  #commit(puts, gone) {
    if (gone.length === 0 && puts.length === 1) {
      return this.#db.execute(UPSERT_ONE, puts[0]);
    }
    if (puts.length === 0 && gone.length === 1) {
      return this.#db.execute("DELETE FROM _state WHERE key = ?", gone[0]);
    }
    return this.#db.transaction(async (tx) => {
      if (puts.length > 0) await tx.executeMany(UPSERT_ONE, puts);
      if (gone.length > 0) await tx.executeMany("DELETE FROM _state WHERE key = ?", gone);
    });
  }

  async close() {
    await this.sync();
    this.#closed = true;
  }
}

// ---------------------------------------------------------------------------
// Live workers: the mailbox, the gate, and eviction
// ---------------------------------------------------------------------------

const LIFECYCLE = new Set(["start", "stop", "alarm", "constructor"]);

class LiveWorker {
  constructor(cls, id, db, instance, state, controller) {
    this.cls = cls;
    this.id = id;
    this.db = db;
    this.instance = instance;
    this.state = state;
    this.controller = controller;
    this.pending = 0;
    // True until `start()` has finished. A worker whose materialization is
    // still running has been idle since the beginning of time by the clock's
    // reckoning, and a sweep that took it would close a database the opener is
    // still using.
    this.starting = true;
    this.lastActive = Date.now();
    this.tail = Promise.resolve();
    this.closing = null;
  }

  /**
   * Runs `method` against this worker, after everything already queued. This is
   * the mailbox and the gate in one place: the result is not handed back until
   * the writes the call made are durable.
   */
  call(method, args) {
    if (this.closing) {
      return Promise.reject(
        new DurableError(
          "this durable worker is closing",
          DurableErrorCode.Shutdown,
        ),
      );
    }
    if (this.pending >= config.mailbox) {
      return Promise.reject(
        new DurableError(
          `${describe(this.cls, this.id)} has ${this.pending} calls waiting, which is its ` +
            "mailbox limit — the caller is ahead of the worker",
          DurableErrorCode.Busy,
        ),
      );
    }
    this.pending++;
    const run = async () => {
      const fn = this.instance[method];
      if (typeof fn !== "function" || LIFECYCLE.has(method)) {
        throw new TypeError(`${describe(this.cls, this.id)} has no method ${String(method)}()`);
      }
      const result = await fn.apply(this.instance, args);
      // The gate. Everything this call wrote is on disk before its caller is
      // told anything at all, so a crash between the two is a call that never
      // returned rather than one that lied.
      await this.state.sync();
      return result;
    };
    const done = this.tail.then(run, run);
    this.tail = done.then(
      () => {},
      () => {},
    );
    return done.finally(() => {
      this.pending--;
      this.lastActive = Date.now();
    });
  }

  async close(reason) {
    if (this.closing) return this.closing;
    this.closing = (async () => {
      await this.tail;
      this.controller.abort(
        new DurableError(`durable worker ${reason}`, DurableErrorCode.Shutdown),
      );
      try {
        if (typeof this.instance.stop === "function") await this.instance.stop(reason);
      } finally {
        await this.state.close();
        await this.db.close();
      }
    })();
    return this.closing;
  }
}

const describe = (cls, id) => `${storageName(cls)}(${JSON.stringify(id)})`;

// Every live worker in this process, keyed by class and id. This map *is* the
// single-writer guarantee: one entry, one instance, one mailbox.
const live = new Map();
const key = (cls, id) => `${storageName(cls)}\u0000${id}`;

// Eviction runs when work arrives rather than on a timer: a repeating timer is
// a reason a process can never exit, and a script that called one durable
// worker would then sit there until it fired.
async function evictIdle() {
  const now = Date.now();
  const closeable = [...live.values()].filter((w) => w.pending === 0 && !w.closing && !w.starting);
  const idle = closeable.filter((w) => now - w.lastActive >= config.evictAfter);
  const overflow = Math.max(0, live.size - idle.length - config.maxLive);
  const lru = closeable
    .filter((w) => !idle.includes(w))
    .sort((a, b) => a.lastActive - b.lastActive)
    .slice(0, overflow);
  for (const worker of [...idle, ...lru]) await evict(worker, "idle");
}

// While a worker is closing it is already out of `live`, so nothing finds it —
// which is the point, since it must take no further calls. What must not happen
// is the *next* materialization opening a second connection to the same file
// while the first is still flushing, so the closing is left where the opener
// looks for it.
const closing = new Map();

async function evict(worker, reason) {
  const k = key(worker.cls, worker.id);
  live.delete(k);
  const done = (async () => {
    try {
      await worker.close(reason);
    } finally {
      await touch(worker.cls, worker.id, worker.state.bytes);
    }
  })();
  closing.set(k, handled(done));
  try {
    await done;
  } finally {
    if (closing.get(k) === done) closing.delete(k);
  }
}

async function touch(cls, id, bytes) {
  if (!registry) return;
  try {
    await retryBusy(() =>
      onCatalog(() =>
        registry.db.execute(
          sql`UPDATE worker SET last_active = ${Date.now()}, bytes = ${bytes}
              WHERE class = ${storageName(cls)} AND id = ${id}`,
        ),
      ),
    );
  } catch {
    // The catalog is bookkeeping, not state. A worker whose `last_active` is
    // behind is a worker with a stale row, not a worker that lost anything.
  }
}

// ---------------------------------------------------------------------------
// Materialization
// ---------------------------------------------------------------------------

// Set only while the runtime is constructing an instance. It is what makes
// `new Room()` from a program throw while `super()` from a subclass constructor
// — which takes no arguments and must not have to pass any — works.
let materializing = null;

let shuttingDown = false;
const inFlight = new Map(); // key -> promise of a LiveWorker

async function materialize(cls, id) {
  if (shuttingDown) {
    fail(DurableErrorCode.Shutdown, "the durable-worker runtime is shutting down");
  }
  const k = key(cls, id);
  const held = live.get(k);
  if (held && !held.closing) {
    held.lastActive = Date.now();
    return held;
  }
  const pending = inFlight.get(k);
  if (pending) return pending;

  const starting = (async () => {
    const db = await registryDb();
    await closing.get(k)?.catch(() => {});
    await evictIdle();

    const name = storageName(cls);
    const file = fileKey(id);
    const dir = `${registry.dir}/${name}/${file.slice(0, 2)}`;
    await mkdir(dir, { recursive: true });
    const path = `${dir}/${file}.db`;

    const store = await open(path);
    let worker;
    try {
      for (const ddl of WORKER_SCHEMA) await retryBusy(() => store.execute(ddl));
      const stored = await one(store, sql`SELECT value FROM _meta WHERE key = 'id'`);
      if (stored === null) {
        await retryBusy(() =>
          store.execute(sql`INSERT INTO _meta (key, value) VALUES ('id', ${id})`),
        );
      } else if (stored.value !== id) {
        fail(
          DurableErrorCode.IdCollision,
          `${JSON.stringify(id)} and ${JSON.stringify(stored.value)} both hash to ${file}`,
        );
      }
      const rows = await (await store.query("SELECT key, value, codec FROM _state")).toArray();
      const state = new State(
        store,
        rows.map((r) => r.toObject()),
      );

      const now = Date.now();
      await retryBusy(() =>
        onCatalog(() =>
          db.execute(
            sql`INSERT INTO worker (class, id, file, created_at, last_active, bytes)
                VALUES (${name}, ${id}, ${file}, ${now}, ${now}, ${state.bytes})
                ON CONFLICT(class, id) DO UPDATE SET last_active = excluded.last_active`,
          ),
        ),
      );

      const controller = new AbortController();
      const ctx = Object.freeze({ id, name, signal: controller.signal });
      materializing = { id, state, ctx };
      let instance;
      try {
        instance = new cls();
      } finally {
        materializing = null;
      }
      worker = new LiveWorker(cls, id, store, instance, state, controller);
      live.set(k, worker);
      // `start()` runs before the mailbox opens rather than as its first entry:
      // every caller is already waiting on this materialization, so there is
      // nothing to get ahead of, and a `start` that throws must leave no worker
      // behind rather than one whose first call reports somebody else's failure.
      try {
        if (typeof instance.start === "function") await instance.start();
      } finally {
        worker.starting = false;
      }
    } catch (e) {
      live.delete(k);
      await store.close().catch(() => {});
      throw e;
    }
    return worker;
  })();

  inFlight.set(k, starting);
  try {
    return await starting;
  } finally {
    inFlight.delete(k);
  }
}

// ---------------------------------------------------------------------------
// The base class, and the reference a program actually holds
// ---------------------------------------------------------------------------

const NAME_PATTERN = /^[A-Za-z0-9_-]{1,64}$/;
const names = new Map(); // storage name -> class

function storageName(cls) {
  const name = cls.durableName ?? cls.name;
  if (typeof name !== "string" || !NAME_PATTERN.test(name)) {
    throw new TypeError(
      `${cls.name || "a durable worker"} needs a storage name matching ${NAME_PATTERN} — ` +
        'set `static durableName = "…"`',
    );
  }
  const claimed = names.get(name);
  if (claimed === undefined) names.set(name, cls);
  else if (claimed !== cls) {
    throw new TypeError(
      `two classes both store as ${JSON.stringify(name)} — give one a distinct ` +
        "`static durableName`",
    );
  }
  return name;
}

function checkId(id) {
  if (typeof id !== "string") throw new TypeError("a durable worker id must be a string");
  if (id.length === 0 || id.length > 512) {
    throw new TypeError("a durable worker id must be 1 to 512 characters");
  }
  if (id.includes("\u0000")) throw new TypeError("a durable worker id must not contain NUL");
  return id;
}

const NOT_A_METHOD = new Set(["then", "catch", "finally", "toJSON", "constructor", "inspect"]);

// A reference is a proxy rather than a generated object: the methods it forwards
// are the class's, and a class that gains one later should not need this file to
// have known about it.
function reference(cls, id) {
  const methods = new Map();
  return new Proxy(Object.freeze({ id }), {
    get(target, property) {
      if (property === "id") return id;
      // A reference must not be a thenable: `await ref` would otherwise call a
      // method named `then` on the worker, which is a hang wearing an await's
      // clothes. Nor may it answer to the names every printer and serializer
      // probes for before it prints something.
      if (typeof property !== "string" || NOT_A_METHOD.has(property)) return undefined;
      let call = methods.get(property);
      if (!call) {
        call = async (...args) => {
          // Arguments and results cross by structured clone even though nothing
          // crosses a thread yet. What may be passed is then the same rule
          // everywhere, rather than one that tightens the day a worker moves to
          // a shard.
          const sent = args.map((a) => structuredClone(a));
          // A worker can be closed between being materialized and being called
          // — an idle sweep on somebody else's call is enough. That is this
          // layer's business, not the caller's, so it is materialized again
          // rather than reported. Once: a second refusal is a real one.
          for (let attempt = 0; ; attempt++) {
            const worker = await materialize(cls, id);
            try {
              return structuredClone(await worker.call(property, sent));
            } catch (e) {
              if (attempt > 0 || e?.code !== DurableErrorCode.Shutdown || shuttingDown) throw e;
            }
          }
        };
        methods.set(property, call);
      }
      return call;
    },
    set() {
      throw new TypeError("a durable worker reference is read-only");
    },
  });
}

/**
 * The base class of every durable worker.
 *
 * Extend it, add methods, and address one by id. The runtime materializes it on
 * first use, runs one call at a time against it, and closes it when it has been
 * idle — its state outliving all of that.
 */
class DurableWorker {
  #state;
  #ctx;

  constructor() {
    if (materializing === null) {
      throw new TypeError(
        "a durable worker is addressed, not constructed — use " +
          `${new.target?.name ?? "TheClass"}.get(id)`,
      );
    }
    this.#state = materializing.state;
    this.#ctx = materializing.ctx;
  }

  /** This worker's key/value state. */
  get state() {
    return this.#state;
  }

  /** The id it was addressed by. */
  get id() {
    return this.#ctx.id;
  }

  /** `{ id, name, signal }` — `signal` aborts when this worker is being closed,
   * so long work can stop rather than be abandoned. */
  get ctx() {
    return this.#ctx;
  }

  /** A reference to the worker of this class with `id`. Nothing is opened until
   * a method is called on it. */
  static get(id) {
    storageName(this);
    return reference(this, checkId(id));
  }

  /** Closes the worker if it is live, then deletes its state for good. */
  static async delete(id) {
    checkId(id);
    const name = storageName(this);
    const db = await registryDb();
    const held = live.get(key(this, id));
    if (held) await evict(held, "deleted");
    const file = fileKey(id);
    const dir = `${registry.dir}/${name}/${file.slice(0, 2)}`;
    for (const suffix of ["", "-wal", "-shm"]) {
      await remove(`${dir}/${file}.db${suffix}`).catch(() => {});
    }
    const result = await retryBusy(() =>
      onCatalog(() => db.execute(sql`DELETE FROM worker WHERE class = ${name} AND id = ${id}`)),
    );
    return result.changes > 0;
  }

  /** The ids of this class's workers that have state, most recently active
   * first. Reads the catalog — a worker need not be live to be listed. */
  static async list({ limit = 100, after } = {}) {
    const name = storageName(this);
    const db = await registryDb();
    const rows = await (
      await onCatalog(() =>
        db.query(
          sql`SELECT id, created_at, last_active, bytes FROM worker
              WHERE class = ${name} AND (${after ?? null} IS NULL OR last_active < ${after ?? null})
              ORDER BY last_active DESC LIMIT ${positive(limit, "limit")}`,
        ),
      )
    ).toArray();
    return rows.map((row) => {
      const r = row.toObject();
      // A live worker's row is behind by construction — the catalog is written
      // when it opens and when it closes, not on every write — so what is live
      // answers for itself.
      const held = live.get(`${name}\u0000${r.id}`);
      return {
        id: r.id,
        createdAt: new Date(r.created_at),
        lastActive: new Date(held?.lastActive ?? r.last_active),
        bytes: held ? held.state.bytes : r.bytes,
        live: held !== undefined,
      };
    });
  }
}

Object.defineProperty(DurableWorker.prototype, Symbol.toStringTag, {
  value: "DurableWorker",
  configurable: true,
});

// ---------------------------------------------------------------------------
// Shutdown
// ---------------------------------------------------------------------------

/**
 * Closes every live worker — flushing what they wrote, running their `stop()` —
 * and gives up this process's lock on the directory.
 *
 * A durable worker's results are gated on their writes, so an abrupt exit loses
 * nothing that was acknowledged; this is how a process *asks* to stop rather
 * than being stopped, which is what gives `stop()` a chance to run at all.
 */
async function shutdown() {
  shuttingDown = true;
  try {
    // One at a time, not `Promise.all`: closing a database checkpoints its WAL,
    // and the engine has been seen to panic when many do so at once.
    for (const worker of [...live.values()]) await evict(worker, "shutdown");
    if (registry) await registry.db.close();
  } finally {
    registry = null;
    shuttingDown = false;
    started = false;
  }
}

export { DurableWorker, configure, shutdown, DurableError, DurableErrorCode };
export default { DurableWorker, configure, shutdown, DurableError, DurableErrorCode };
