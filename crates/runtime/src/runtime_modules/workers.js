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
  // How many times a failing `alarm()` is retried before it is given up on and
  // reported, and the longest the scheduler will sleep between looks at the
  // catalog. The ceiling matters because the machine's clock can move.
  alarmRetries: 5,
  alarmPoll: 60_000,
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

// `limit` is the resident ceiling, and it is the *keys'* ceiling: a document
// lives in a table and is read when it is asked for, so bounding it would bound
// the thing collections exist to hold.
function encode(value, what, limit = Infinity) {
  if (typeof serialize !== "function") {
    fail(DurableErrorCode.StateFormat, "this build cannot serialize durable state");
  }
  let bytes;
  try {
    bytes = serialize(value);
  } catch (e) {
    throw new TypeError(`${what} cannot be stored — ${e?.message ?? e}`, { cause: e });
  }
  if (bytes.byteLength > limit) {
    fail(
      DurableErrorCode.StateTooLarge,
      `${what}: ${bytes.byteLength} bytes is over the ${limit}-byte limit for one value`,
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
     next_alarm  INTEGER,
     PRIMARY KEY (class, id)
   )`,
  // What makes an alarm findable without opening every worker's database: the
  // scheduler asks this index which one is next, and opens exactly that one.
  `CREATE INDEX IF NOT EXISTS worker_alarm ON worker (next_alarm) WHERE next_alarm IS NOT NULL`,
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
  // A catalog written before alarms existed has no column for them. The table
  // is an index over the workers' own files rather than the state itself, so
  // widening it is the whole migration.
  const columns = await (await onCatalog(() => db.query("PRAGMA table_info(worker)"))).toArray();
  if (!columns.some((c) => c.toObject().name === "next_alarm")) {
    await retryBusy(() =>
      onCatalog(() => db.execute("ALTER TABLE worker ADD COLUMN next_alarm INTEGER")),
    );
  }
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

// A time may be given as a Date or as milliseconds since the epoch — the two
// spellings every other timer in this runtime accepts.
function whenMs(when) {
  const at = when instanceof Date ? when.getTime() : when;
  if (typeof at !== "number" || !Number.isFinite(at)) {
    throw new TypeError("an alarm time must be a Date or a number of milliseconds");
  }
  return Math.trunc(at);
}

// Whether two byte strings are the same. Short-circuits on length, which is
// what almost every different value differs in.
function same(a, b) {
  if (a === undefined || a.byteLength !== b.byteLength) return false;
  for (let i = 0; i < a.byteLength; i++) if (a[i] !== b[i]) return false;
  return true;
}

// One at a time, in the order they were asked for.
function serial(chain, work) {
  const next = chain.tail.then(() => work());
  chain.tail = next.then(
    () => {},
    () => {},
  );
  return next;
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
  #alarm = null; // ms, or null
  #index; // (at, early) => Promise — keeps the catalog's copy in step
  #handler = null; // whether the class has an alarm() method; null until known
  #collections; // declared name -> { index, unique }
  #opened = new Map(); // name -> Collection

  #attempt = 0;
  #chain = { tail: Promise.resolve() };
  #txChain = { tail: Promise.resolve() };
  #tx = false;

  constructor(db, { rows, alarm, attempt, index, collections }) {
    this.#db = db;
    this.#collections = collections;
    this.#alarm = alarm;
    this.#attempt = attempt;
    this.#index = index;
    /**
     * When this worker's `alarm()` should next run.
     *
     * The time is stored beside the state, so it survives a restart the same
     * way the state does — and a worker with an alarm set is woken by the
     * scheduler whether or not anybody addresses it.
     */
    this.alarm = Object.freeze({
      /** The time set, or `null`. Synchronous, like the rest of the state. */
      get: () => this.#alarm && new Date(this.#alarm),
      /** Sets it; resolves when it is durable. A time in the past runs at once. */
      set: (when) => this.#setAlarm(when),
      /** Unsets it. */
      delete: () => this.#setAlarm(null),
    });
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
    if (this.#tx) {
      // Waiting for the transaction from inside it would be waiting for
      // ourselves; writing into it is what "make it durable" means here, and it
      // becomes true at the commit.
      await this.#flushWithin();
      return;
    }
    while (this.#flushing || this.#dirty.size > 0) {
      await (this.#flushing ?? this.#schedule());
    }
  }

  /**
   * Runs `work` with this worker's database to itself.
   *
   * A connection is one conversation — the catalog learned that the hard way —
   * and a worker has two callers for its own: the flush behind a `set`, and
   * whatever a collection is doing. They are not ordered by anything else, so
   * they are ordered here. Re-entrant, because a collection call inside a
   * `transaction` is already holding it.
   */
  run(work) {
    // Two chains, and which one a statement joins depends on whether a
    // transaction is open. Outside one, *nothing* bypasses: a flush and a
    // collection write both want this connection and neither knows about the
    // other, which is exactly the pair that put two statements on it and
    // panicked the engine. Inside one, the outer queue is already held by the
    // transaction, so joining it again would be a wait on itself — the inner
    // chain keeps that work one-at-a-time without waiting for the holder.
    return serial(this.#tx ? this.#txChain : this.#chain, work);
  }

  /**
   * A named collection: rows in a table of their own, queried rather than held.
   *
   * Only what the class declared in `static schema` exists — a name it does not
   * know is a typo, and a typo that quietly made a table would be a second
   * store nobody meant to have.
   */
  collection(name) {
    this.#open();
    const declared = this.#collections.get(name);
    if (declared === undefined) {
      const known = [...this.#collections.keys()];
      throw new TypeError(
        `no collection ${JSON.stringify(name)} is declared on this durable worker` +
          (known.length ? ` — it has ${known.map((n) => JSON.stringify(n)).join(", ")}` : ""),
      );
    }
    let held = this.#opened.get(name);
    if (held === undefined) {
      held = new Collection(this, this.#db, name, declared);
      this.#opened.set(name, held);
    }
    return held;
  }

  /**
   * Runs `work` inside a transaction over everything this worker stores — its
   * keys and its collections alike. It commits when `work` returns and rolls
   * back when it throws.
   */
  transaction(work) {
    this.#open();
    return this.run(async () => {
      this.#tx = true;
      try {
        return await this.#db.transaction(async () => {
          const value = await work();
          // Anything `set` left behind goes in before the commit, so a
          // transaction really does cover both halves of this worker's storage.
          await this.#flushWithin();
          return value;
        });
      } finally {
        this.#tx = false;
      }
    });
  }

  // The dirty keys, written on the connection the transaction is already
  // holding. Not a flush: no queue, no scheduling, and no promise of its own.
  #flushWithin() {
    const keys = [...this.#dirty];
    this.#dirty.clear();
    if (keys.length === 0) return Promise.resolve();
    const { puts, gone } = this.#pending(keys);
    return retryBusy(() => this.run(() => this.#commit(puts, gone)));
  }

  // The catalog is written *before* the worker's own file and again after, and
  // that order is the whole reliability argument: the catalog is only an index,
  // so an entry that is early causes a wake-up that finds nothing to do — while
  // one that is late is an alarm that never fires. Between the two writes a
  // crash therefore leaves the index early, never late. (`MIN` on the way in,
  // exact on the way out, so moving an alarm *later* still cannot lose the one
  // it replaced.)
  async #setAlarm(when) {
    this.#open();
    if (when !== null && this.#handler === false) {
      throw new TypeError(
        "this durable worker has no alarm() method, so an alarm set on it could never run",
      );
    }
    const at = when === null ? null : whenMs(when);
    if (at !== null) await this.#index(at, true);
    await retryBusy(() =>
      this.run(() =>
        at === null
          ? this.#db.execute("DELETE FROM _meta WHERE key = 'alarm'")
          : this.#db.execute(
              sql`INSERT INTO _meta (key, value) VALUES ('alarm', ${String(at)})
                  ON CONFLICT(key) DO UPDATE SET value = excluded.value`,
            ),
      ),
    );
    this.#alarm = at;
    await this.#index(at, false);
  }

  /** Told once, when the instance exists: whether its class can answer an
   * alarm at all. Setting one on a class that cannot is refused at the `set`,
   * where the mistake is, rather than by a scheduler nobody is watching. */
  set alarmHandler(present) {
    this.#handler = present;
  }

  /** How many times the pending alarm has failed. Stored, so a retry count is
   * not lost with the process that was counting. */
  get alarmAttempt() {
    return this.#attempt;
  }

  async setAlarmAttempt(n) {
    this.#attempt = n;
    await retryBusy(() =>
      this.run(() =>
        n === 0
          ? this.#db.execute("DELETE FROM _meta WHERE key = 'alarm_attempt'")
          : this.#db.execute(
              sql`INSERT INTO _meta (key, value) VALUES ('alarm_attempt', ${String(n)})
                  ON CONFLICT(key) DO UPDATE SET value = excluded.value`,
            ),
      ),
    );
  }

  #write(key, value) {
    const bytes = encode(value, `state.set(${JSON.stringify(key)})`, config.valueLimit);
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
    // Inside a transaction there is no flush to schedule: the write goes into
    // the transaction with everything else, and becomes true when it commits —
    // which is what a statement inside a transaction has always meant.
    if (this.#tx) return this.#flushWithin();
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
    const { puts, gone } = this.#pending(keys);
    try {
      await retryBusy(() => this.run(() => this.#commit(puts, gone)));
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

  #pending(keys) {
    const puts = [];
    const gone = [];
    for (const key of keys) {
      const held = this.#values.get(key);
      if (held) puts.push([key, held.encoded, CODEC_STRUCTURED_CLONE]);
      else gone.push([key]);
    }
    return { puts, gone };
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
// Collections
// ---------------------------------------------------------------------------

// A collection is a table of its own: the document as bytes, and the fields the
// class declared as real columns beside it, indexed. Which is the whole trade —
// what is declared can be queried and sorted by the database, and what is not is
// inside a blob the database cannot see into, because the blob is what keeps a
// `Date` a `Date`.
const NAME = /^[A-Za-z0-9_]{1,64}$/;

const table = (name) => `"c_${name}"`;
const column = (field) => `"f_${field}"`;

// Parses `static schema` into what the storage layer needs, and refuses
// anything it could not build a table from — at the first use of the class,
// which is where a schema typo should be reported.
const parsed = new WeakMap();

// Parsed once per class, and parsed at `get()` rather than at the first open:
// a schema is a literal in the source, so a typo in one should be reported by
// the line that addresses the worker, not by the filesystem later.
function schemaOf(cls) {
  let held = parsed.get(cls);
  if (held === undefined) {
    held = parseSchema(cls);
    parsed.set(cls, held);
  }
  return held;
}

function parseSchema(cls) {
  const schema = cls.schema;
  const out = new Map();
  if (schema === undefined) return out;
  if (schema === null || typeof schema !== "object") {
    throw new TypeError(`${storageName(cls)}: static schema must be an object`);
  }
  const { collections = {}, ...rest } = schema;
  const unknown = Object.keys(rest);
  if (unknown.length > 0) {
    throw new TypeError(
      `${storageName(cls)}: static schema has no ${JSON.stringify(unknown[0])} — it takes { collections }`,
    );
  }
  for (const [name, declared] of Object.entries(collections)) {
    if (!NAME.test(name)) {
      throw new TypeError(`${storageName(cls)}: ${JSON.stringify(name)} is not a collection name`);
    }
    const { index = [], unique = [], ...extra } = declared ?? {};
    const left = Object.keys(extra);
    if (left.length > 0) {
      throw new TypeError(
        `${storageName(cls)}.${name}: no such option ${JSON.stringify(left[0])} — it takes { index, unique }`,
      );
    }
    const fields = new Map();
    for (const [list, isUnique] of [
      [index, false],
      [unique, true],
    ]) {
      if (!Array.isArray(list)) {
        throw new TypeError(`${storageName(cls)}.${name}: index and unique must be arrays`);
      }
      for (const field of list) {
        if (typeof field !== "string" || !NAME.test(field)) {
          throw new TypeError(
            `${storageName(cls)}.${name}: ${JSON.stringify(field)} is not a field name — ` +
              "a declared field is a plain top-level property",
          );
        }
        if (field === "id" || field === "doc" || field === "codec") {
          throw new TypeError(
            `${storageName(cls)}.${name}: ${JSON.stringify(field)} is a column this table already has`,
          );
        }
        fields.set(field, isUnique || (fields.get(field) ?? false));
      }
    }
    out.set(name, fields);
  }
  return out;
}

// What a declared field may be, and what it becomes in its column. A column
// exists to be compared and sorted, so what goes in it has to be a value SQLite
// can order — and a document that would put something else there is a mistake
// worth reporting at the insert rather than at the query.
function promoted(value, field) {
  if (value === undefined || value === null) return null;
  if (typeof value === "string" || typeof value === "number") return value;
  if (typeof value === "boolean") return value ? 1 : 0;
  if (value instanceof Date) return value.getTime();
  if (typeof value === "bigint") return Number(value);
  throw new TypeError(
    `the declared field ${JSON.stringify(field)} must be a string, number, boolean, Date or null — ` +
      `got ${value?.constructor?.name ?? typeof value}`,
  );
}

// The operators a `where` may use. Everything else is a bare value, which is
// equality — the case almost every query is.
const OPERATORS = {
  eq: "=",
  ne: "!=",
  gt: ">",
  gte: ">=",
  lt: "<",
  lte: "<=",
};

// What the declared schema hashes to. Compared against the hash stored in the
// worker's own file, so an unchanged schema costs a string comparison and a
// changed one costs the difference — the reason a wake-up is not a migration.
const schemaHash = (collections) =>
  hash(
    "blake3",
    JSON.stringify([...collections].map(([name, fields]) => [name, [...fields].sort()])),
    "hex",
  );

// Brings one worker's file up to what its class declares: the tables, the
// columns for declared fields, and the indexes over them. It runs at hydration,
// which is what makes a deploy's first wake the slow one — the alternative is a
// migration step somebody has to remember to run against every worker.
//
// Nothing is ever dropped. A field or a collection removed from the schema
// leaves its column and its table alone: the class stopped asking for it, which
// is not the same as the data being unwanted, and a schema edit that deletes
// rows is a schema edit nobody can undo.
async function ensureCollections(store, collections, stored) {
  const want = schemaHash(collections);
  if (stored === want) return want;
  for (const [name, fields] of collections) {
    await retryBusy(() =>
      store.execute(
        `CREATE TABLE IF NOT EXISTS ${table(name)} (
           id    TEXT PRIMARY KEY,
           doc   BLOB NOT NULL,
           codec INTEGER NOT NULL
         )`,
      ),
    );
    const info = await (await store.query(`PRAGMA table_info(${table(name)})`)).toArray();
    const have = new Set(info.map((c) => c.toObject().name));
    const added = [];
    for (const field of fields.keys()) {
      if (have.has(`f_${field}`)) continue;
      await retryBusy(() =>
        store.execute(`ALTER TABLE ${table(name)} ADD COLUMN ${column(field)}`),
      );
      added.push(field);
    }
    // A column added to a table that already has rows is null in all of them,
    // and a query over it would then quietly miss every document written before
    // the field was declared. So the documents are read once and the column
    // filled in — the cost of declaring a field late, paid where it is visible.
    if (added.length > 0) {
      const rows = await (await store.query(`SELECT id, doc, codec FROM ${table(name)}`)).toArray();
      const updates = rows.map((row) => {
        const r = row.toObject();
        const doc = decode(r.doc, r.codec, name);
        return [...added.map((field) => promoted(doc?.[field], field)), r.id];
      });
      if (updates.length > 0) {
        await retryBusy(() =>
          store.executeMany(
            `UPDATE ${table(name)} SET ${added.map((f) => `${column(f)} = ?`).join(", ")} WHERE id = ?`,
            updates,
          ),
        );
      }
    }
    for (const [field, unique] of fields) {
      await retryBusy(() =>
        store.execute(
          `CREATE ${unique ? "UNIQUE " : ""}INDEX IF NOT EXISTS "c_${name}_f_${field}" ` +
            `ON ${table(name)} (${column(field)})`,
        ),
      );
    }
  }
  await retryBusy(() =>
    store.execute(
      sql`INSERT INTO _meta (key, value) VALUES ('schema', ${want})
          ON CONFLICT(key) DO UPDATE SET value = excluded.value`,
    ),
  );
  return want;
}

class Collection {
  #state;
  #db;
  #name;
  #fields;

  constructor(state, db, name, fields) {
    this.#state = state;
    this.#db = db;
    this.#name = name;
    this.#fields = fields;
  }

  /** The collection's name, as the class declared it. */
  get name() {
    return this.#name;
  }

  /**
   * Stores `doc`, returning its id — `doc.id` when it has one, a fresh UUID
   * when it does not. An id that is already there is replaced.
   */
  async insert(doc) {
    const [row] = this.#rows([doc]);
    await retryBusy(() => this.#state.run(() => this.#db.execute(this.#upsert(), row)));
    return row[0];
  }

  /** Several documents in one statement, which is one crossing and one commit. */
  async insertMany(docs) {
    const rows = this.#rows(docs);
    if (rows.length === 0) return [];
    await retryBusy(() => this.#state.run(() => this.#db.executeMany(this.#upsert(), rows)));
    return rows.map((row) => row[0]);
  }

  /** The document with `id`, or `undefined`. */
  async get(id) {
    const row = await this.#state.run(() =>
      one(this.#db, `SELECT doc, codec FROM ${table(this.#name)} WHERE id = ?`, [text(id)]),
    );
    return row === null ? undefined : decode(row.doc, row.codec, `${this.#name}/${id}`);
  }

  /**
   * Applies `patch` — an object to merge, or a function of the document — and
   * returns what is now stored, or `undefined` if there was nothing to change.
   */
  async update(id, patch) {
    const current = await this.get(id);
    if (current === undefined) return undefined;
    const next = typeof patch === "function" ? await patch(current) : { ...current, ...patch };
    if (next === undefined || next === null) return undefined;
    next.id = current.id ?? text(id);
    const [row] = this.#rows([next]);
    await retryBusy(() => this.#state.run(() => this.#db.execute(this.#upsert(), row)));
    return next;
  }

  /** Removes it. Resolves to whether there was one. */
  async delete(id) {
    const result = await retryBusy(() =>
      this.#state.run(() =>
        this.#db.execute(`DELETE FROM ${table(this.#name)} WHERE id = ?`, [text(id)]),
      ),
    );
    return result.changes > 0;
  }

  /** Removes everything `where` selects, and answers how many that was. */
  async deleteWhere(where) {
    const { text: clause, params } = this.#where(where);
    const result = await retryBusy(() =>
      this.#state.run(() =>
        this.#db.execute(`DELETE FROM ${table(this.#name)}${clause}`, params),
      ),
    );
    return result.changes;
  }

  /**
   * A query. Nothing runs until it is iterated, `toArray`-ed, or counted.
   *
   *     await state.collection("messages")
   *       .find({ authorId: "u1", ts: { gte: since } })
   *       .sort({ ts: "desc" })
   *       .limit(20)
   *       .toArray();
   *
   * Every field named has to be one the class declared, because the rest are
   * inside a blob the database cannot see into. `{ scan: true }` says to read
   * the documents and filter them here instead, which is honest work on a small
   * collection and a full read on a large one.
   */
  find(where = {}, options = {}) {
    return new Query(this.#state, this.#db, this.#name, this.#fields, where, options);
  }

  /** How many documents `where` selects. */
  count(where = {}, options = {}) {
    return this.find(where, options).count();
  }

  #upsert() {
    const fields = [...this.#fields.keys()];
    const columns = ["id", "doc", "codec", ...fields.map(column)];
    return (
      `INSERT INTO ${table(this.#name)} (${columns.join(", ")}) ` +
      `VALUES (${columns.map(() => "?").join(", ")}) ` +
      `ON CONFLICT(id) DO UPDATE SET ${columns
        .slice(1)
        .map((c) => `${c} = excluded.${c}`)
        .join(", ")}`
    );
  }

  #rows(docs) {
    return [...docs].map((doc) => {
      if (doc === null || typeof doc !== "object") {
        throw new TypeError("a document must be an object");
      }
      const id = typeof doc.id === "string" && doc.id.length > 0 ? doc.id : crypto.randomUUID();
      const stored = doc.id === id ? doc : { ...doc, id };
      const bytes = encode(stored, `the document ${JSON.stringify(id)} in ${this.#name}`);
      return [
        id,
        bytes,
        CODEC_STRUCTURED_CLONE,
        ...[...this.#fields.keys()].map((field) => promoted(stored[field], field)),
      ];
    });
  }

  #where(where) {
    return whereClause(this.#fields, where, this.#name, false);
  }
}

// Shared by `find` and `deleteWhere`: the SQL a `where` becomes, and the
// parameters beside it.
function whereClause(fields, where, name, scan) {
  if (where === null || typeof where !== "object") {
    throw new TypeError("a query is an object of fields to match");
  }
  const parts = [];
  const params = [];
  const inJs = [];
  for (const [field, test] of Object.entries(where)) {
    if (field !== "id" && !fields.has(field)) {
      if (scan) {
        inJs.push([field, test]);
        continue;
      }
      throw new TypeError(
        `${name}: ${JSON.stringify(field)} is not a declared field, so it is inside the ` +
          "document rather than beside it — declare it in `static schema`, or pass { scan: true } " +
          "to read the documents and filter here",
      );
    }
    const col = field === "id" ? "id" : column(field);
    if (test !== null && typeof test === "object" && !(test instanceof Date)) {
      for (const [op, operand] of Object.entries(test)) {
        if (op === "in") {
          if (!Array.isArray(operand) || operand.length === 0) {
            throw new TypeError(`${name}.${field}: "in" takes a non-empty array`);
          }
          parts.push(`${col} IN (${operand.map(() => "?").join(", ")})`);
          params.push(...operand.map((v) => promoted(v, field)));
          continue;
        }
        const sqlOp = OPERATORS[op];
        if (sqlOp === undefined) {
          throw new TypeError(
            `${name}.${field}: no such comparison ${JSON.stringify(op)} — ` +
              `expected one of: ${Object.keys(OPERATORS).join(", ")}, in`,
          );
        }
        parts.push(`${col} ${sqlOp} ?`);
        params.push(promoted(operand, field));
      }
      continue;
    }
    const value = field === "id" ? text(test) : promoted(test, field);
    parts.push(value === null ? `${col} IS NULL` : `${col} = ?`);
    if (value !== null) params.push(value);
  }
  return { text: parts.length ? ` WHERE ${parts.join(" AND ")}` : "", params, inJs };
}

const text = (id) => {
  if (typeof id !== "string") throw new TypeError("a document id must be a string");
  return id;
};

class Query {
  #state;
  #db;
  #name;
  #fields;
  #where;
  #scan;
  #order = [];
  #limit = null;
  #offset = 0;

  constructor(state, db, name, fields, where, { scan = false } = {}) {
    this.#state = state;
    this.#db = db;
    this.#name = name;
    this.#fields = fields;
    this.#where = where;
    this.#scan = scan === true;
  }

  /** `sort({ ts: "desc" })`, and as many keys as you like. */
  sort(order) {
    for (const [field, direction] of Object.entries(order)) {
      if (field !== "id" && !this.#fields.has(field) && !this.#scan) {
        throw new TypeError(
          `${this.#name}: cannot sort by ${JSON.stringify(field)}, which is not a declared field`,
        );
      }
      if (direction !== "asc" && direction !== "desc") {
        throw new TypeError(`${this.#name}: a sort direction is "asc" or "desc"`);
      }
      this.#order.push([field, direction]);
    }
    return this;
  }

  limit(n) {
    this.#limit = positive(n, "limit");
    return this;
  }

  offset(n) {
    this.#offset = positive(n + 1, "offset") - 1;
    return this;
  }

  /** Every match, as an array. */
  async toArray() {
    const out = [];
    for await (const doc of this) out.push(doc);
    return out;
  }

  /** The first match, or `null`. */
  async first() {
    for await (const doc of this) return doc;
    return null;
  }

  /** How many there are. Counted by the database unless this is a scan. */
  async count() {
    if (this.#scan) return (await this.toArray()).length;
    const { text: clause, params } = whereClause(this.#fields, this.#where, this.#name, false);
    const row = await this.#state.run(() =>
      one(this.#db, `SELECT count(*) AS n FROM ${table(this.#name)}${clause}`, params),
    );
    return row.n;
  }

  /** Matches, one at a time, pulled a batch at a time from the database. */
  /** Matches, one at a time. */
  async *[Symbol.asyncIterator]() {
    for (const doc of await this.#fetch()) yield doc;
  }

  // The whole result, read in one turn on the connection.
  //
  // Deliberately *not* streamed while the caller iterates: a cursor holds the
  // connection open across the loop body, and a loop body that touched this
  // worker's state — an `await state.set` inside a `for await` is the obvious
  // thing to write — would then be waiting on a connection its own iteration is
  // holding. `.limit()` is what bounds a large collection; the iterator stays
  // async so streaming can arrive later without changing what callers wrote.
  async #fetch() {
    const {
      text: clause,
      params,
      inJs,
    } = whereClause(this.#fields, this.#where, this.#name, this.#scan);
    const indexed = this.#order.filter(([f]) => f === "id" || this.#fields.has(f));
    const jsOrder = this.#order.filter(([f]) => f !== "id" && !this.#fields.has(f));
    const orderBy = indexed.length
      ? ` ORDER BY ${indexed
          .map(([f, d]) => `${f === "id" ? "id" : column(f)} ${d === "desc" ? "DESC" : "ASC"}`)
          .join(", ")}`
      : "";
    // A query the database can answer entirely is limited by the database. One
    // that needs documents read cannot be, since what is filtered here is not
    // known there — so the limit is applied after the filtering instead.
    const pushDown = inJs.length === 0 && jsOrder.length === 0;
    const tail =
      pushDown && this.#limit !== null ? ` LIMIT ${this.#limit} OFFSET ${this.#offset}` : "";
    const rows = await this.#state.run(async () =>
      (
        await this.#db.query(
          `SELECT doc, codec FROM ${table(this.#name)}${clause}${orderBy}${tail}`,
          params,
        )
      ).toArray(),
    );
    let docs = rows.map((row) => {
      const r = row.toObject();
      return decode(r.doc, r.codec, this.#name);
    });
    if (inJs.length > 0) docs = docs.filter((doc) => matches(doc, inJs));
    if (jsOrder.length > 0) {
      docs.sort((a, b) => {
        for (const [field, direction] of this.#order) {
          const x = a?.[field] instanceof Date ? a[field].getTime() : a?.[field];
          const y = b?.[field] instanceof Date ? b[field].getTime() : b?.[field];
          if (x === y) continue;
          const less =
            x === undefined || x === null ? true : y === undefined || y === null ? false : x < y;
          return (less ? -1 : 1) * (direction === "desc" ? -1 : 1);
        }
        return 0;
      });
    }
    if (pushDown) return docs;
    const end = this.#limit === null ? docs.length : this.#offset + this.#limit;
    return docs.slice(this.#offset, end);
  }
}

// The part of a `where` the database could not answer, applied to a document
// that has been read.
function matches(doc, inJs) {
  for (const [field, test] of inJs) {
    const value = doc?.[field];
    if (test !== null && typeof test === "object" && !(test instanceof Date)) {
      for (const [op, operand] of Object.entries(test)) {
        if (op === "in") {
          if (!operand.some((o) => same_value(value, o))) return false;
          continue;
        }
        const x = value instanceof Date ? value.getTime() : value;
        const y = operand instanceof Date ? operand.getTime() : operand;
        const ok =
          op === "eq"
            ? x === y
            : op === "ne"
              ? x !== y
              : op === "gt"
                ? x > y
                : op === "gte"
                  ? x >= y
                  : op === "lt"
                    ? x < y
                    : op === "lte"
                      ? x <= y
                      : undefined;
        if (ok === undefined) {
          throw new TypeError(`no such comparison ${JSON.stringify(op)}`);
        }
        if (!ok) return false;
      }
      continue;
    }
    if (!same_value(value, test)) return false;
  }
  return true;
}

const same_value = (a, b) =>
  a instanceof Date && b instanceof Date ? a.getTime() === b.getTime() : a === b;

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
    return this.enqueue(async () => {
      const fn = this.instance[method];
      if (typeof fn !== "function" || LIFECYCLE.has(method)) {
        throw new TypeError(`${describe(this.cls, this.id)} has no method ${String(method)}()`);
      }
      return fn.apply(this.instance, args);
    });
  }

  /**
   * Puts `work` in this worker's mailbox: it runs after everything already
   * queued, and its result is not handed back until the writes it made are
   * durable. Both a method call and an alarm arrive this way, which is what
   * makes an alarm unable to interleave with a call.
   */
  enqueue(work) {
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
      const result = await work();
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

// Writes the catalog's copy of a worker's alarm. `early` is the first of the two
// writes `State.#setAlarm` makes: it only ever moves the time *earlier*, so the
// window between them cannot lose an alarm that was already indexed.
async function indexAlarm(name, id, at, early) {
  const db = await registryDb();
  await retryBusy(() =>
    onCatalog(() =>
      db.execute(
        early
          ? sql`UPDATE worker SET next_alarm = MIN(COALESCE(next_alarm, ${at}), ${at})
                WHERE class = ${name} AND id = ${id}`
          : sql`UPDATE worker SET next_alarm = ${at} WHERE class = ${name} AND id = ${id}`,
      ),
    ),
  );
  wakeScheduler();
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
    const collections = schemaOf(cls);
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
      const meta = new Map(
        (await (await store.query("SELECT key, value FROM _meta")).toArray()).map((r) => {
          const m = r.toObject();
          return [m.key, m.value];
        }),
      );
      const alarm = meta.has("alarm") ? Number(meta.get("alarm")) : null;
      await ensureCollections(store, collections, meta.get("schema"));
      const state = new State(store, {
        rows: rows.map((r) => r.toObject()),
        alarm,
        attempt: Number(meta.get("alarm_attempt") ?? 0),
        index: (at, early) => indexAlarm(name, id, at, early),
        collections,
      });

      const now = Date.now();
      await retryBusy(() =>
        onCatalog(() =>
          db.execute(
            // The alarm is written here as well as by `state.alarm.set`: the
            // worker's own file is the truth and the catalog only an index, so
            // opening one is the moment to make the index agree again — which
            // is how a crash between the two writes is repaired.
            sql`INSERT INTO worker (class, id, file, created_at, last_active, bytes, next_alarm)
                VALUES (${name}, ${id}, ${file}, ${now}, ${now}, ${state.bytes}, ${alarm})
                ON CONFLICT(class, id) DO UPDATE SET last_active = excluded.last_active,
                                                     next_alarm = excluded.next_alarm`,
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
      state.alarmHandler = typeof instance.alarm === "function";
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
    schemaOf(this);
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
// Alarms
// ---------------------------------------------------------------------------

// A process that wants to run alarms says so. There is no hidden timer that
// starts itself, for the reason eviction has none: a timer is a claim on the
// process staying alive, and a script that set an alarm for tomorrow would
// otherwise not exit until tomorrow. A server calls this once; a script that
// only *sets* alarms never has to.
let scheduler = null;

/**
 * Starts servicing alarms: due workers are woken and their `alarm()` runs.
 *
 * `classes` is the list of durable-worker classes **this process can run**, and
 * it is required. A class is not something the runtime can discover — importing
 * a module that defines one proves nothing about whether this deployment is the
 * one meant to service it — and a scheduler that quietly serviced only the
 * classes something had happened to address would fire an alarm on a busy
 * process and not on an idle one. Anything scheduled for a class not listed
 * here is left exactly as it is, for the process that does list it.
 *
 * While it is running the process stays alive, which is the point — a service
 * that has work scheduled has a reason to be up. `stop()` gives that up and
 * resolves once the sweep in flight has finished.
 *
 * `onError` hears about an alarm that failed for the last time, and about a
 * worker that could not be opened. Without one, both go to `console.error`,
 * because a scheduled job failing silently is how a queue loses work.
 */
function startAlarms({ classes, onError, batch = 32 } = {}) {
  if (!Array.isArray(classes) || classes.length === 0) {
    throw new TypeError(
      "startAlarms({ classes }): name the durable worker classes this process runs alarms for, " +
        "e.g. startAlarms({ classes: [Cart] })",
    );
  }
  if (onError !== undefined && typeof onError !== "function") {
    throw new TypeError("startAlarms({ onError }): onError must be a function");
  }
  const named = classes.map((cls) => {
    if (typeof cls !== "function" || !Object.prototype.isPrototypeOf.call(DurableWorker, cls)) {
      throw new TypeError("startAlarms({ classes }): each entry must be a DurableWorker subclass");
    }
    return storageName(cls);
  });
  if (scheduler) {
    // Returning the running one would quietly ignore the classes this call
    // named, which is a scheduler that does not do what its caller asked.
    throw new TypeError(
      "alarms are already being serviced in this process — stop that scheduler before " +
        "starting another, or name every class in one call",
    );
  }
  const state = {
    timer: null,
    stopped: false,
    running: null,
    again: false,
    onError,
    names: named,
    batch: positive(batch, "batch"),
  };
  state.handle = Object.freeze({
    /** Stops servicing alarms. Resolves when the sweep in flight has finished. */
    async stop() {
      state.stopped = true;
      clearTimeout(state.timer);
      state.timer = null;
      if (scheduler === state) scheduler = null;
      await state.running;
    },
    /** Whether it is still running. */
    get running() {
      return !state.stopped;
    },
  });
  scheduler = state;
  tick(state);
  return state.handle;
}

// Called whenever an alarm is written, so one set for *sooner* than the timer
// that is already waiting does not have to wait for it.
function wakeScheduler() {
  const state = scheduler;
  if (!state || state.stopped) return;
  if (state.running) {
    state.again = true;
    return;
  }
  clearTimeout(state.timer);
  state.timer = null;
  tick(state);
}

function tick(state) {
  if (state.stopped || state.running) return;
  state.running = (async () => {
    let delay = config.alarmPoll;
    try {
      await fireDue(state);
      const next = await nextAlarm(state);
      if (next !== null) delay = Math.min(delay, Math.max(0, next - Date.now()));
    } catch (e) {
      report(state, e, "the alarm scheduler failed");
    }
    return delay;
  })();
  state.running.then((delay) => {
    state.running = null;
    if (state.stopped) return;
    if (state.again) {
      state.again = false;
      tick(state);
      return;
    }
    state.timer = setTimeout(() => tick(state), delay);
  });
}

// The `class IN (…)` on both queries is what keeps a class this process does
// not run out of its way entirely: not fetched, not skipped in a loop, and —
// since such a row is overdue for ever — never the reason the scheduler wakes
// up again immediately.
const placeholders = (names) => names.map(() => "?").join(", ");

async function nextAlarm(state) {
  const db = await registryDb();
  const row = await onCatalog(() =>
    one(
      db,
      `SELECT MIN(next_alarm) AS at FROM worker
       WHERE next_alarm IS NOT NULL AND class IN (${placeholders(state.names)})`,
      state.names,
    ),
  );
  return row?.at ?? null;
}

async function fireDue(state) {
  const db = await registryDb();
  const rows = await (
    await onCatalog(() =>
      db.query(
        `SELECT class, id FROM worker
         WHERE next_alarm IS NOT NULL AND next_alarm <= ? AND class IN (${placeholders(state.names)})
         ORDER BY next_alarm LIMIT ?`,
        [Date.now(), ...state.names, state.batch],
      ),
    )
  ).toArray();
  for (const row of rows) {
    if (state.stopped) break;
    const due = row.toObject();
    await fire(state, names.get(due.class), due.id);
  }
}

// One worker's alarm, run through its mailbox — so it cannot interleave with a
// call, and its writes are gated exactly as a call's are.
async function fire(state, cls, id) {
  let worker;
  try {
    worker = await materialize(cls, id);
  } catch (e) {
    report(state, e, `could not open ${describe(cls, id)} for its alarm`);
    return;
  }
  try {
    await worker.enqueue(() => run(state, worker));
  } catch (e) {
    report(state, e, `the alarm on ${describe(cls, id)} could not be run`);
  }
}

async function run(state, worker) {
  const at = worker.state.alarm.get();
  if (at === null || at.getTime() > Date.now()) {
    // The catalog said due and the worker's own file disagrees. The file is the
    // truth — this is the crash window the two-step write leaves — so the index
    // is corrected and nothing runs.
    await indexAlarm(storageName(worker.cls), worker.id, at === null ? null : at.getTime(), false);
    return;
  }
  // Cleared before the handler runs, so an `alarm()` that sets the next one is
  // the natural way to repeat, and a handler that sets nothing is not woken
  // again. A failure puts one back.
  await worker.state.alarm.delete();
  try {
    await worker.instance.alarm();
    if (worker.state.alarmAttempt !== 0) await worker.state.setAlarmAttempt(0);
  } catch (e) {
    const attempt = worker.state.alarmAttempt + 1;
    if (attempt > config.alarmRetries) {
      await worker.state.setAlarmAttempt(0);
      report(
        state,
        e,
        `the alarm on ${describe(worker.cls, worker.id)} failed ${attempt} times and was given up on`,
      );
      return;
    }
    await worker.state.setAlarmAttempt(attempt);
    // Unless the handler scheduled the next one itself on its way out, in which
    // case that is the time it asked for and a retry would overwrite it.
    if (worker.state.alarm.get() === null) {
      await worker.state.alarm.set(Date.now() + backoff(attempt));
    }
  }
}

// 1s, 2s, 4s … capped at five minutes. Deliberately not configurable: the
// number of retries is the interesting knob, and a schedule with two of them is
// two things to explain.
const backoff = (attempt) => Math.min(1000 * 2 ** (attempt - 1), 300_000);

function report(state, error, context) {
  if (state.onError) {
    try {
      state.onError(error, context);
      return;
    } catch {
      // An `onError` that throws is not a reason to lose the failure it was
      // told about.
    }
  }
  console.error(`${context}:`, error);
}

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
    await scheduler?.handle.stop();
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

export { DurableWorker, configure, startAlarms, shutdown, DurableError, DurableErrorCode };
export default {
  DurableWorker,
  configure,
  startAlarms,
  shutdown,
  DurableError,
  DurableErrorCode,
};
