
// ---------------------------------------------------------------------------
// Isolate (DECISIONS D227)
// ---------------------------------------------------------------------------
//
// Appended to workers.js: one module, one scope.
//
// An `Isolate` is a small agent for code this program does not control — a
// customer's rule, a plugin, code a model wrote. It runs on a pool of threads
// shared with every other isolate, holds no capability and can be granted
// none: it computes, and reaches the outside only through the `host`
// functions it is handed. `Worker` stays the tool for parallel work and for
// code that needs the outside world.
//
// The isolate's end of the channel is the envelope in the worker prelude
// (worker.js), which says what each frame means. This is the parent's end.

const ISOLATE_TERMINATED = "ERR_ISOLATE_TERMINATED";

// Captured at load, as the durable workers capture theirs.
const isolateSerialize = (frame) => globalThis.__internal.transfer.serializeMessage(frame, []);
const isolateDeserialize = (bytes) => globalThis.__structuredDeserialize(bytes)[0];

const ISOLATE_ERROR_CLASSES = {
  Error,
  EvalError,
  RangeError,
  ReferenceError,
  SyntaxError,
  TypeError,
  URIError,
};

/** An error in the pieces a structured clone keeps of it. */
function isolateErrorPieces(error) {
  const isObject = error !== null && typeof error === "object";
  return {
    name: isObject && error.name !== undefined ? String(error.name) : "Error",
    message: isObject && error.message !== undefined ? String(error.message) : String(error),
    stack: isObject && typeof error.stack === "string" ? error.stack : "",
    code: isObject && typeof error.code === "string" ? error.code : undefined,
  };
}

/** The standard class of an error that crossed, its name, stack and code. */
function isolateRebuildError(pieces) {
  const Class = Object.hasOwn(ISOLATE_ERROR_CLASSES, pieces?.name)
    ? ISOLATE_ERROR_CLASSES[pieces.name]
    : Error;
  const error = new Class(String(pieces?.message ?? "isolate call failed"));
  // The host names its own failures by code — `ERR_ISOLATE_CPU_LIMIT` — and
  // a code is what `ERR_ISOLATE_TERMINATED` is read by, so they arrive as one.
  if (typeof pieces?.code !== "string" && /^ERR_ISOLATE_[A-Z_]+$/.test(pieces?.name ?? "")) {
    error.code = pieces.name;
    if (pieces.stack) error.stack = pieces.stack;
    return error;
  }
  if (pieces?.name && pieces.name !== error.name) error.name = pieces.name;
  if (pieces?.stack) error.stack = pieces.stack;
  if (typeof pieces?.code === "string") error.code = pieces.code;
  return error;
}

function isolateTerminated(why) {
  const error = new Error(why);
  error.code = ISOLATE_TERMINATED;
  return error;
}

// A single-handler slot (`onmessage = fn`), as on `Worker`.
function isolateHandlerSlot(target, type) {
  let current = null;
  return {
    get: () => current,
    set(value) {
      if (typeof current === "function") target.removeEventListener(type, current);
      current = typeof value === "function" ? value : null;
      if (current !== null) target.addEventListener(type, current);
    },
    enumerable: true,
    configurable: true,
  };
}

// The least an isolate starts in: restoring the runtime's prelude needs a
// heap of about this size (7 MB fails, measured on debug and release builds),
// so less is refused here, by name, rather than failing to start later.
const ISOLATE_MIN_MEMORY_MB = 8;

function isolateMemory(value) {
  if (value === undefined || value === null) return 0;
  const mb = Number(value);
  if (!Number.isInteger(mb) || mb <= 0) {
    throw new TypeError(
      'Isolate option "memory" must be a positive whole number of megabytes, ' +
        `not ${JSON.stringify(value)}`,
    );
  }
  if (mb < ISOLATE_MIN_MEMORY_MB) {
    throw new RangeError(
      `Isolate option "memory" must be at least ${ISOLATE_MIN_MEMORY_MB} MB: ` +
        "an isolate needs that much to start",
    );
  }
  return mb;
}

function isolateHost(value) {
  if (value === undefined || value === null) return Object.create(null);
  if (typeof value !== "object") {
    throw new TypeError('Isolate option "host" must be an object of functions');
  }
  const host = Object.create(null);
  for (const [name, fn] of Object.entries(value)) {
    if (typeof fn !== "function") {
      throw new TypeError(`Isolate option "host": ${JSON.stringify(name)} is not a function`);
    }
    host[name] = fn;
  }
  return host;
}

function isolateCpuTime(value) {
  if (value === undefined || value === null) return 0;
  const ms = Number(value);
  if (!Number.isInteger(ms) || ms <= 0) {
    throw new TypeError(
      'Isolate option "cpuTime" must be a positive whole number of milliseconds, ' +
        `not ${JSON.stringify(value)}`,
    );
  }
  return ms;
}

const ISOLATE_OPTIONS = new Set(["name", "host", "memory", "cpuTime"]);
const ISOLATE_CODE_KEYS = new Set(["source", "name"]);

/**
 * Refuses any key of a `{ source }` argument but `source` and `name`. A limit
 * written there would otherwise be dropped, and the code would run without
 * it: the one mistake an isolate's options must never turn into silently.
 */
function isolateCheckCode(code, opts) {
  for (const key of Object.keys(code)) {
    if (ISOLATE_CODE_KEYS.has(key)) continue;
    if (key === "permissions") {
      throw new TypeError(
        "an Isolate is computation only and takes no permissions — pass it what it " +
          'needs through "host" functions, or use a Worker',
      );
    }
    if (ISOLATE_OPTIONS.has(key)) {
      throw new TypeError(
        `"${key}" is an Isolate option, not part of its code: ` +
          `new Isolate({ source }, { ${key} })`,
      );
    }
    throw new TypeError(
      `unknown key ${JSON.stringify(key)} in Isolate code — it takes { source, name }`,
    );
  }
  if (code.name !== undefined && opts.name !== undefined) {
    throw new TypeError(
      'Isolate "name" given twice — in the code and in the options; give it once',
    );
  }
}

/**
 * A small, sandboxed agent on a shared pool of threads.
 *
 * ```js
 * const rule = new Isolate({ source: code, name: "discount" }, { memory: 16 });
 * const { discount } = await rule.exports;
 * await discount(order);
 * rule.terminate();
 * ```
 */
class Isolate extends EventTarget {
  #id = null;
  #ready;
  #terminated = false;
  #referenced = false;
  #pending = [];
  #host;
  #calls = new Map();
  #nextCall = 1;
  #exports;
  #settleExports;

  /**
   * @param {URL | string | { source: string, name?: string }} code The module
   *   to run: a file's URL, or its source text.
   * @param {{ name?: string, host?: Record<string, Function>, memory?: number, cpuTime?: number }} [options]
   */
  constructor(code, options = {}) {
    super();
    if (arguments.length < 1) throw new TypeError("Isolate requires a module URL or { source }");
    const opts = options ?? {};
    if (typeof opts !== "object") throw new TypeError("Isolate options must be an object");
    for (const key of Object.keys(opts)) {
      if (key === "permissions") {
        throw new TypeError(
          "an Isolate is computation only and takes no permissions — pass it what it " +
            'needs through "host" functions, or use a Worker',
        );
      }
      if (!ISOLATE_OPTIONS.has(key)) {
        throw new TypeError(`unknown Isolate option ${JSON.stringify(key)}`);
      }
    }
    if (code !== null && typeof code === "object" && !(code instanceof URL)) {
      isolateCheckCode(code, opts);
    }
    this.#host = isolateHost(opts.host);
    const memory = isolateMemory(opts.memory);
    const cpuTime = isolateCpuTime(opts.cpuTime);

    let start;
    if (code !== null && typeof code === "object" && !(code instanceof URL)) {
      if (typeof code.source !== "string") {
        throw new TypeError("Isolate code must be a module URL or { source: string }");
      }
      const name = code.name === undefined ? String(opts.name ?? "") : String(code.name);
      start = () => __ops.isolate_spawn_source(code.source, name, memory, cpuTime);
    } else {
      const url = new URL(String(code), __ops.worker_base()).href;
      const name = opts.name === undefined ? "" : String(opts.name);
      start = () => __ops.isolate_spawn_file(url, name, memory, cpuTime);
    }

    this.#exports = new Promise((resolve, reject) => {
      this.#settleExports = { resolve, reject };
    });
    // Nobody may ever ask for the exports of an isolate used only through
    // messages; its failure is reported through `error`, not as a rejection
    // no one could have caught.
    this.#exports.catch(() => {});

    Object.defineProperty(this, "onmessage", isolateHandlerSlot(this, "message"));
    Object.defineProperty(this, "onerror", isolateHandlerSlot(this, "error"));

    this.#setReferenced(true);
    this.#ready = this.#start(start);
  }

  async #start(start) {
    try {
      this.#id = await start();
    } catch (error) {
      queueMicrotask(() => this.#fail(isolateErrorPieces(error), error));
      return null;
    }
    // Synchronously, so a message posted from a microtask cannot overtake
    // those posted before the spawn resolved (as on `Worker`).
    const pending = this.#pending;
    this.#pending = [];
    if (this.#terminated) {
      __ops.isolate_terminate(this.#id);
      return this.#id;
    }
    for (const bytes of pending) __ops.isolate_post(this.#id, bytes);
    this.#pump();
    return this.#id;
  }

  async #pump() {
    try {
      for (;;) {
        if (this.#terminated) return;
        let event;
        try {
          event = await __ops.isolate_recv(this.#id);
        } catch {
          return;
        }
        if (event === null || event === undefined) return;
        if (event.type === "message") {
          let frame;
          try {
            frame = isolateDeserialize(event.data);
          } catch {
            this.dispatchEvent(new MessageEvent("messageerror"));
            continue;
          }
          this.#frame(frame);
        } else if (event.type === "error") {
          this.#fail(event.data);
        } else if (event.type === "close") {
          return;
        }
      }
    } finally {
      this.#end(isolateTerminated("the isolate has ended"));
    }
  }

  #frame(frame) {
    if (!Array.isArray(frame)) return;
    switch (frame[0]) {
      case 0:
        this.dispatchEvent(new MessageEvent("message", { data: frame[1] }));
        return;
      case 2: {
        const call = this.#calls.get(frame[1]);
        if (call === undefined) return;
        this.#calls.delete(frame[1]);
        if (frame[2]) call.resolve(frame[3]);
        else call.reject(isolateRebuildError(frame[3]));
        return;
      }
      case 3:
        this.#hostCall(frame[1], frame[2], frame[3]);
        return;
      case 5: {
        const exports = Object.create(null);
        for (const name of frame[1]) {
          exports[name] = (...args) => this.#call(name, args);
        }
        this.#settleExports.resolve(Object.freeze(exports));
        return;
      }
    }
  }

  async #hostCall(id, name, args) {
    let reply;
    try {
      const fn = Object.hasOwn(this.#host, name) ? this.#host[name] : undefined;
      if (fn === undefined) {
        throw new TypeError(`this isolate was handed no host function named ${JSON.stringify(name)}`);
      }
      reply = [4, id, true, await fn(...args)];
    } catch (error) {
      reply = [4, id, false, isolateErrorPieces(error)];
    }
    if (this.#terminated) return;
    let bytes;
    try {
      bytes = isolateSerialize(reply);
    } catch (error) {
      bytes = isolateSerialize([4, id, false, isolateErrorPieces(error)]);
    }
    __ops.isolate_post(this.#id, bytes);
  }

  #call(name, args) {
    if (this.#terminated) {
      return Promise.reject(isolateTerminated("the isolate has been terminated"));
    }
    return new Promise((resolve, reject) => {
      const id = this.#nextCall++;
      let bytes;
      try {
        bytes = isolateSerialize([1, id, name, args]);
      } catch (error) {
        reject(error);
        return;
      }
      this.#calls.set(id, { resolve, reject });
      this.#send(bytes);
    });
  }

  #send(bytes) {
    if (this.#terminated) return;
    if (this.#id === null) this.#pending.push(bytes);
    else __ops.isolate_post(this.#id, bytes);
  }

  /** Every call still waiting, and the exports if they never came, end with `error`. */
  #end(error) {
    this.#terminated = true;
    this.#setReferenced(false);
    this.#pending = [];
    this.#settleExports.reject(error);
    for (const call of this.#calls.values()) call.reject(error);
    this.#calls.clear();
  }

  #fail(pieces, original) {
    const error = original ?? isolateRebuildError(pieces);
    this.#end(error);
    const event = new ErrorEvent("error", {
      message: pieces?.message ?? String(error.message),
      filename: pieces?.filename,
      lineno: pieces?.lineno,
      colno: pieces?.colno,
      error,
      cancelable: true,
    });
    // Unclaimed, it goes to the console, as a worker's does — never escalated
    // into this agent's own failure.
    if (this.dispatchEvent(event)) globalThis.console.error(pieces?.stack || event.message);
  }

  #setReferenced(on) {
    if (this.#referenced === on) return;
    this.#referenced = on;
    __ops.worker_ref(on ? 1 : -1);
  }

  /**
   * The module's exported functions, as async functions that call into the
   * isolate. Resolves once the module has evaluated; rejects if it fails to.
   * @type {Promise<Record<string, (...args: unknown[]) => Promise<unknown>>>}
   */
  get exports() {
    return this.#exports;
  }

  /** Sends `message` to the isolate's `onmessage`. */
  postMessage(message) {
    if (this.#terminated) return;
    this.#send(isolateSerialize([0, message]));
  }

  /** Stops the isolate now. Calls still waiting reject with `ERR_ISOLATE_TERMINATED`. */
  terminate() {
    if (this.#terminated) return;
    this.#end(isolateTerminated("the isolate has been terminated"));
    if (this.#id !== null) {
      __ops.isolate_terminate(this.#id);
    }
    // Still starting: `#start` sees the flag once the spawn resolves.
  }

  /**
   * Builds `count` isolates ahead of time, so the next `new Isolate()` calls
   * start without building one — a pool of warm isolates. Resolves with how
   * many are waiting. An isolate given its own `memory` can't use one, since a
   * memory ceiling is fixed when an isolate is made; it starts as usual.
   * @param {number} count
   * @returns {Promise<number>}
   */
  static prepare(count) {
    const n = Number(count);
    if (!Number.isInteger(n) || n < 1) {
      return Promise.reject(
        new TypeError(`Isolate.prepare needs a positive whole number, not ${JSON.stringify(count)}`),
      );
    }
    return __ops.isolate_prepare(n);
  }

  /** Stop this isolate from keeping the process alive. */
  unref() {
    this.#setReferenced(false);
  }

  /** Undoes {@link unref}. */
  ref() {
    if (!this.#terminated) this.#setReferenced(true);
  }
}

Object.defineProperty(Isolate.prototype, Symbol.toStringTag, {
  value: "Isolate",
  configurable: true,
});

/** Only defined inside an isolate; see `runtime:workers` there. */
const host = undefined;

export { Isolate, host };
export default {
  DurableWorker,
  configure,
  startAlarms,
  shutdown,
  DurableError,
  DurableErrorCode,
  Isolate,
  host,
};
