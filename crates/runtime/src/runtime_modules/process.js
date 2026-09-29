// runtime:process — host process info (DECISIONS D24), aligned in spirit with
// the WinterTC CLI-API proposal. An ES module (not a global), backed by ops
// gated on Capability::Env. Values are snapshotted when the module evaluates.

const ops = globalThis.__ops;

// Secret masking (DECISIONS D30): env values whose key matches a secret-bearing
// convention are exposed as a `Secret` rather than a raw string, so they redact
// to "[redacted]" wherever they would otherwise leak — console output, string
// coercion / template literals, and JSON.stringify. The real value is held in a
// module-private WeakMap and is only obtainable via `unmask(...)`. This guards
// against *accidental* logging, not a hostile guest (which can call `unmask`).
const REDACTED = "[redacted]";
// A global-registry symbol the console inspector checks to render "[redacted]"
// without importing this module (console lives in the prelude snapshot).
const REDACTED_MARK = Symbol.for("runtime.secret.redacted");
// The real value, for a deliberate reader in another runtime: module —
// `runtime:system` has to unwrap a Secret before handing it to a child process,
// or the child would receive the literal "[redacted]". Symbol-keyed, so it stays
// invisible to console, JSON.stringify, and string coercion: the accidental
// paths are exactly what masking covers, and this is not one of them.
const REDACTED_VALUE = Symbol.for("runtime.secret.value");
// A key is treated as secret-bearing (case-insensitive) when it either ends in
// `_SECRET(S)`, `_PASSWORD(S)`, `_PASS`, `_KEY(S)`, or `_TOKEN(S)` — the leading
// `_` avoids false hits like MONKEY/BYPASS — or contains `CREDENTIAL(S)` or
// `AUTH` as an underscore-delimited word (so AUTH_TOKEN/API_AUTH match, AUTHOR
// does not). Over-matching a non-secret is harmless: `unmask` still returns it.
const SECRET_KEY =
  /_(?:SECRET|PASSWORD|PASS|KEY|TOKEN)S?$|(?:^|_)(?:CREDENTIAL|AUTH)S?(?:_|$)/i;
const secrets = new WeakMap();

class Secret {
  constructor(value) {
    secrets.set(this, value);
  }
  toString() {
    return REDACTED;
  }
  valueOf() {
    return REDACTED;
  }
  toJSON() {
    return REDACTED;
  }
  [Symbol.toPrimitive]() {
    return REDACTED;
  }
  get [REDACTED_MARK]() {
    return true;
  }
  get [REDACTED_VALUE]() {
    return secrets.get(this);
  }
}

// `unmask(value)`: reveal a `Secret`'s real value. Plain strings pass through
// unchanged, so `unmask(env.ANY)` is always safe regardless of whether the key
// happened to match the secret convention.
function unmask(value) {
  if (typeof value === "string") return value;
  if (value instanceof Secret) return secrets.get(value);
  throw new TypeError("unmask expects a string or a Secret from runtime:process env");
}

// Importing a `runtime:` module must never need a capability — the gate is the
// op, not the import (DECISIONS D26/D38). `env` is the only binding here whose
// value comes from an `Env`-gated op, so it seeds itself on *first access*
// rather than at module evaluation. Under `--deny-env` this module still imports
// (and `exit`, `onSignal`, `permissions` still work); touching `env` is what
// throws. `args` uses the same lazy seeding — not because it needs a capability
// (D65 ungated it) but because it is frozen on first read, and freezing at
// module evaluation would fix it before a worker's arguments arrive.
function seeded(target, fill, onWrite) {
  let done = false;
  const seed = () => {
    if (done) return;
    // Set last: a denial throws out of `fill`, leaving `done` false so the next
    // access retries and throws again rather than exposing a half-filled value.
    fill(target);
    done = true;
  };
  // Every trap that can observe or mutate the target seeds first, so the value
  // is indistinguishable from one built eagerly.
  return new Proxy(target, {
    get: (t, k, r) => (seed(), Reflect.get(t, k, r)),
    set: (t, k, v, r) => (
      seed(), Reflect.set(t, k, onWrite === undefined ? v : onWrite(k, v), r)
    ),
    has: (t, k) => (seed(), Reflect.has(t, k)),
    deleteProperty: (t, k) => (seed(), Reflect.deleteProperty(t, k)),
    ownKeys: (t) => (seed(), Reflect.ownKeys(t)),
    getOwnPropertyDescriptor: (t, k) => (
      seed(), Reflect.getOwnPropertyDescriptor(t, k)
    ),
    // `Object.defineProperty(env, k, { value })` is a write that never reaches
    // the `set` trap, so without this it was the one way to leave a raw
    // non-string in `env` (Node coerces here too).
    defineProperty: (t, k, d) => (
      seed(),
      Reflect.defineProperty(
        t,
        k,
        onWrite !== undefined && d !== undefined && "value" in d
          ? { ...d, value: onWrite(k, d.value) }
          : d,
      )
    ),
    // Without this, an `Object.freeze(env)` before any read would lock an empty
    // target and the later seeding would silently fail.
    preventExtensions: (t) => (seed(), Reflect.preventExtensions(t)),
    // `Object.isFrozen`/`isSealed` ask this *first*, and an untrapped
    // `isExtensible` forwards to a target that has not been seeded yet — so it
    // reported the empty, still-extensible array and `Object.isFrozen(args)`
    // came back false for a value the docs call frozen (and which is frozen,
    // the moment anything reads it).
    isExtensible: (t) => (seed(), Reflect.isExtensible(t)),
  });
}

// `env`: a mutable in-process object seeded from the host snapshot. Reads,
// writes, and deletes work in-process; they do not (yet) propagate to the host
// process or future child processes. Secret-keyed values are wrapped (above).
const env = seeded({}, (target) => {
  // A worker its parent handed an `env` reads that, and needs no capability for
  // it: those values came from the parent, which already held them. Everyone
  // else reads the host environment, which does need `Env` — the call below is
  // the one that throws when it was not granted.
  const provided = ops.process_env_provided();
  for (const [key, value] of provided ?? ops.process_env()) {
    // Masked by the same key convention either way. A parent that unwrapped a
    // Secret to pass it on does not thereby unmask it for the worker.
    target[key] = toEnvValue(key, value);
  }
}, toEnvValue);

// What actually gets stored for an assignment to `env`, applied to the host
// snapshot *and* to anything written later.
//
// The value is coerced to a string, because an environment is a string-to-string
// map and nothing else can ever reach a child process. Storing `env.PORT = 8080`
// verbatim left a *number* in a map that claims to be the environment:
// `typeof env.PORT` was "number", and handing that whole object to
// `new Command(cmd, { env })` threw "must be a string" for a value the program
// had every reason to think it had set correctly. Node and Deno both coerce
// (`String(value)`, so `undefined` becomes "undefined" and a symbol throws, as
// it does anywhere else); this now matches them.
//
// Masking is applied by the same key convention. A key assigned at runtime —
// `env.MY_API_KEY = "…"`, which is how a program threads a value it just
// fetched down to a child — was stored raw, so the same name that arrives masked
// from the environment stayed a plain string when the program set it, and leaked
// in a log line or a `JSON.stringify` like any other. A value that is *already*
// a Secret is left alone rather than wrapped twice — and not coerced either,
// which would stringify the wrapper instead of what it holds.
function toEnvValue(key, value) {
  if (value instanceof Secret) return value;
  // Template coercion, not `String(value)`: the two agree on everything except a
  // symbol, which `String` renders as "Symbol(x)" and this rejects — the same
  // TypeError Node and Deno raise, rather than storing a description of the
  // symbol as though it were the value.
  const string = `${value}`;
  return SECRET_KEY.test(String(key)) ? new Secret(string) : string;
}

// `args`: the program arguments after the runtime binary and the script/-e code.
// Frozen once seeded, so it is read-only exactly as an eager `Object.freeze`
// would have made it.
const args = seeded([], (target) => {
  target.push(...ops.process_args());
  Object.freeze(target);
});

// `platform`: the host OS — std::env::consts::OS values ("linux"/"macos"/...).
const platform = ops.process_platform();

// `arch`: the host CPU architecture — std::env::consts::ARCH values
// ("x86_64"/"aarch64"/"arm"/...).
const arch = ops.process_arch();

// `cwd()`: the current working directory (a function — it can change).
function cwd() {
  return ops.process_cwd();
}

// `exit(code = 0)`: record the exit code and halt execution.
function exit(code = 0) {
  ops.process_exit(Number(code) | 0);
}

// ---- standard output -------------------------------------------------------
//
// `console.log` formats a value, appends a newline, and goes wherever the host
// pointed it. That is the right shape for a log line and the wrong one for a
// **display**: a spinner is a carriage return and no newline, a progress bar
// rewrites the line it is already on, and neither is expressible as a series of
// log calls. So the streams themselves are here.
//
//   import { stdout } from "runtime:process";
//
//   if (stdout.isTTY) {
//     stdout.write(`\r${bar(done / total, stdout.columns ?? 60)}`);
//   } else {
//     console.log(`${done}/${total}`);
//   }
//
// **`isTTY` is not a nicety.** A spinner redrawn with `\r` into a log file is a
// file of spinner frames, and colour escapes in a pipe are noise in somebody's
// grep. A program that cannot ask is a program that draws into both.
//
// Ungated, like `console.log`: writing to the stream this program was started
// with reaches nothing the program was not already handed, so `--deny-all`
// still leaves it able to say what it is doing.

const encoder = new TextEncoder();

// Text or bytes. A string is UTF-8; a view or buffer is written exactly as it
// is, because a program drawing a box already knows which bytes it means.
function toBytes(chunk) {
  if (typeof chunk === "string") return encoder.encode(chunk);
  if (chunk instanceof ArrayBuffer) return new Uint8Array(chunk);
  if (ArrayBuffer.isView(chunk)) {
    return new Uint8Array(chunk.buffer, chunk.byteOffset, chunk.byteLength);
  }
  throw new TypeError("write expects a string, ArrayBuffer, or ArrayBufferView");
}

function stream(name) {
  return Object.freeze({
    /** The name of this stream: "stdout" or "stderr". */
    name,
    // Exactly these bytes, flushed. No newline is added — that is the whole
    // difference from console.log.
    write(chunk) {
      ops.process_write(name, toBytes(chunk));
      return undefined;
    },
    // Whether this stream is attached to a terminal. A getter rather than a
    // snapshot: a program may be handed a different stream than the one it
    // started with, and the answer costs a syscall.
    get isTTY() {
      return ops.process_is_terminal(name);
    },
    // The terminal's width and height, or `undefined` when there is no
    // terminal or the host cannot say. Asked of the terminal, not of
    // `$COLUMNS`, which a shell exports to itself and which is stale the
    // moment the window is dragged.
    get columns() {
      return size()?.[0];
    },
    get rows() {
      return size()?.[1];
    },
  });
}

function size() {
  return ops.process_terminal_size() ?? undefined;
}

const stdout = stream("stdout");
const stderr = stream("stderr");

// ---- standard input (D143) -------------------------------------------------
//
//   import { stdin } from "runtime:process";
//
//   const name = await stdin.question("Name? ");
//   for await (const line of stdin.lines()) { … }
//
// Every way of reading takes from one buffer on the host, so bytes read past a
// line end by one of them are the next one's — a `question()` followed by
// `lines()` loses nothing. Ungated, like `stdout`: this is the stream the
// program was started with. A worker's reads throw; a process has one input.

const lineDecoder = new TextDecoder();

// A line's bytes as text, without its terminator (`\n`, `\r\n`, or the lone
// `\r` raw mode's Enter sends).
function lineText(bytes) {
  let end = bytes.length;
  if (end > 0 && bytes[end - 1] === 0x0a) end--;
  if (end > 0 && bytes[end - 1] === 0x0d) end--;
  return lineDecoder.decode(bytes.subarray(0, end));
}

let rawMode = false;
let readable;

const stdin = Object.freeze({
  /** The name of this stream: "stdin". */
  name: "stdin",
  get isTTY() {
    return ops.process_is_terminal("stdin");
  },
  // Raw bytes as they arrive. One stream, made on first use, pulling only when
  // read (a high-water mark of 0): an eager pull would take input out of the
  // shared buffer that nobody asked this stream for.
  get readable() {
    readable ??= new ReadableStream(
      {
        async pull(controller) {
          const chunk = await ops.process_stdin_read(false);
          if (chunk === null) controller.close();
          else controller.enqueue(chunk);
        },
      },
      { highWaterMark: 0 },
    );
    return readable;
  },
  // Lines without their terminators, until the end of input.
  async *lines() {
    for (;;) {
      const line = await ops.process_stdin_read(true);
      if (line === null) return;
      yield lineText(line);
    }
  },
  // Writes `query` to standard error — where a question belongs when standard
  // output may be a file — and resolves with the next line, or `null` at the
  // end of input. It works on a pipe too, which is what makes a program that
  // asks questions scriptable.
  async question(query = "") {
    if (query !== "") ops.process_write("stderr", toBytes(`${query}`));
    const line = await ops.process_stdin_read(true);
    return line === null ? null : lineText(line);
  },
  // Keypresses as they are typed: no echo, no line editing, and ^C arrives as
  // the byte 0x03 rather than as a signal. The runtime gives the terminal back
  // when the program ends, however it ends. Throws when stdin is not a
  // terminal. Returns `stdin`, as Node's does.
  setRawMode(on) {
    const raw = Boolean(on);
    ops.process_stdin_set_raw(raw);
    rawMode = raw;
    return stdin;
  },
  get isRaw() {
    return rawMode;
  },
});

// ---- signals ---------------------------------------------------------------
//
// Gated on Capability::Signals, not Env: watching a signal suppresses its
// default action, so it is the privilege to decline to die on request rather
// than a read of process state.
//
// The runtime owns no loop, so delivery is pulled: one pump awaits
// `signal_next` and dispatches, and it runs only while something is watched.
// That pending op is also what keeps the program alive to receive a signal —
// the same behaviour as Node and Deno, and the point of installing a handler at
// all. Remove the last handler and the pump is released, so a program that
// stops listening can still exit.

const handlers = new Map(); // name -> Set<function>
let pumping = false;

async function pump() {
  pumping = true;
  try {
    for (;;) {
      const name = await ops.signal_next();
      if (name === null) break; // nothing watched any more — release the loop
      const listeners = handlers.get(name);
      if (listeners === undefined) continue;
      // Iterate a copy: a handler may legitimately call offSignal (a one-shot
      // shutdown hook is the obvious case) while the set is being walked.
      for (const handler of [...listeners]) {
        try {
          handler(name);
        } catch (e) {
          // One bad handler must not stop the others or kill the pump; report
          // it the way any other unhandled failure is reported.
          reportError(e);
        }
      }
    }
  } finally {
    pumping = false;
  }
}

function checkName(name) {
  if (typeof name !== "string") throw new TypeError("signal name must be a string");
  return name;
}

// `signals`: the signal names this platform can actually deliver. Reading it
// needs the capability but watches nothing.
function signals() {
  return ops.signal_available();
}

// `onSignal(name, handler)`: run `handler` when `name` arrives. The first
// handler for a signal starts watching it, which suppresses its default action.
function onSignal(name, handler) {
  checkName(name);
  if (typeof handler !== "function") throw new TypeError("signal handler must be a function");
  let listeners = handlers.get(name);
  if (listeners === undefined) {
    // Watch before recording the handler, so a signal this platform cannot
    // deliver throws instead of registering a handler that would never fire.
    ops.signal_watch(name);
    listeners = new Set();
    handlers.set(name, listeners);
  }
  listeners.add(handler);
  if (!pumping) pump();
}

// `offSignal(name, handler)`: remove a handler. Removing the last one for a
// signal stops watching it and restores the default action.
function offSignal(name, handler) {
  checkName(name);
  const listeners = handlers.get(name);
  if (listeners === undefined) return;
  listeners.delete(handler);
  if (listeners.size === 0) {
    handlers.delete(name);
    ops.signal_unwatch(name);
  }
}

// ---- permissions -----------------------------------------------------------
//
// What this process is allowed to reach (DECISIONS D38). The policy is fixed at
// launch by esrun's --deny-all / --deny-* flags (or by the embedder's capability
// set), so this is introspection only: there is nothing to request, and no
// prompt to await. Hence a synchronous boolean rather than the promise-returning
// shape runtimes with interactive prompts use.
//
// The backing op is ungated, so this answers even under --deny-all — which is
// the policy under which a program most needs to ask.

// The denial vocabulary, identical to the --deny-<name> flag suffixes, read
// from the host rather than transcribed: the authoritative list is Rust-side
// (Capability::HOST_FACING), and a copy here would be a second place to forget
// when a capability is added. Used only to reject typos in has() rather than
// answering them.
const PERMISSIONS = Object.freeze(ops.permission_names());

const permissions = Object.freeze({
  /** The names this process may not use — `[]` when nothing is denied. */
  get denied() {
    return Object.freeze(ops.process_permissions_denied());
  },
  /**
   * Whether `name` is available. An unknown name throws rather than answering
   * `false`: a typo'd check would otherwise read as a denial and silently take
   * the degraded path forever.
   *
   * Takes **one** argument. A per-value query (`has("read", "/etc/passwd")`)
   * throws rather than answering about the capability and ignoring the value,
   * which would be the same lie the CLI refuses to tell when it rejects a flag
   * it cannot enforce. Whether a *particular* path or host is reachable is
   * decided by the deployment's `--allow-<name>=<list>`, and answered by making
   * the call: the runtime checks a path only after resolving it, so any answer
   * given in advance could be stale by the time the call happens.
   */
  has(name, ...rest) {
    if (rest.length > 0) {
      throw new TypeError(
        "permissions.has() takes one argument: a per-value check " +
          `(has(${JSON.stringify(name)}, …)) is not supported. Scoping is set by the ` +
          "deployment (--allow-<name>=<list>); to learn whether one path, host or " +
          "program is allowed, perform the operation and catch the denial " +
          "(code ERR_PERMISSION_DENIED).",
      );
    }
    if (!PERMISSIONS.includes(name)) {
      throw new TypeError(
        `'${name}' is not a permission name (expected one of: ${PERMISSIONS.join(", ")})`,
      );
    }
    return !ops.process_permissions_denied().includes(name);
  },
});

// How much memory *this agent's isolate* is using, in bytes.
//
//   heapUsed   what V8 has allocated and not collected
//   heapLimit  this isolate's ceiling — `--max-heap`, or a worker's own
//   external   ArrayBuffers and other memory V8 holds outside its heap
//
// Per isolate, not per process: a worker has its own heap and its own ceiling,
// so `heapLimit - heapUsed` answers "how close is *this* agent to being killed
// by the heap guard?" — which is the question, and one nothing else could
// answer. The process's resident set is about the agents around you and lives
// behind the `diagnostics` capability, in `metrics().process`.
//
// Ungated for the reason `platform` and `args` are: it reports only what the
// caller could discover about itself anyway, by allocating until it stops.
function memoryUsage() {
  const [heapUsed, heapLimit, external] = globalThis.__heap_bytes();
  return { heapUsed, heapLimit, external };
}

// CPU milliseconds **this agent's thread** has used, since the agent started.
//
// A worker is its own OS thread, so this is what answers "is it *me* burning the
// CPU?" — the question you cannot ask of a process-wide number. Read it twice
// and divide by the wall time between to get a utilisation.
//
// Total, not split into user and system time: the split needs Mach on macOS,
// where getting a struct layout wrong is a memory-safety bug rather than a wrong
// number, so it is reported nowhere rather than on two platforms out of three.
//
// The process's total is `metrics().process.cpu` in runtime:diagnostics, behind
// the `diagnostics` capability, because it is about the agents around you.
function cpuTime() {
  return ops.process_cpu_time();
}

// Milliseconds since **this agent** started.
//
// `performance.now()` is not this: a worker is handed its parent's clock, so it
// counts from when the process's runtime was built and reads the same in every
// agent. This counts from when this one did.
function uptime() {
  return ops.process_uptime();
}

// Whether a timer holds the process open, spelled as Deno spells it:
// `setTimeout` returns a number here, as on the web, so there is no timer
// object to call `unref()` on as in Node. An unreferenced timer still fires
// while anything else keeps the process running; alone, it does not keep it.
// For a heartbeat or a periodic flush that should not be why a program never
// exits.
//
// A cleared or already-fired timer is left alone: there is nothing to change.
// Ungated: it decides only when *this* program may end.
function timerRef(name, id, referenced) {
  if (typeof id !== "number") {
    throw new TypeError(`${name} needs the id setTimeout or setInterval returned`);
  }
  globalThis.__timer_ref(id, referenced);
}
function unrefTimer(id) {
  timerRef("unrefTimer", id, false);
}
function refTimer(id) {
  timerRef("refTimer", id, true);
}

// `parseArgs(config)`: Node's `util.parseArgs`, reading this program's `args`
// by default (DECISIONS D142). Same options, same tokenizer, same result shape
// and the same `e.code`s, so a parser written for `node:util` ports by changing
// its import. Ungated, like `args`: it reads nothing else.
//
// Three phases, as in Node: split the arguments into tokens, check and store
// each token against the declared options, then fill in defaults.

function argError(code, message) {
  const err = new TypeError(message);
  err.code = code;
  return err;
}

function own(object, key) {
  return Object.prototype.hasOwnProperty.call(object, key) ? object[key] : undefined;
}

function optionGet(options, name, key) {
  return Object.prototype.hasOwnProperty.call(options, name)
    ? own(options[name], key)
    : undefined;
}

function checkType(value, name, expected, ok) {
  if (!ok(value)) {
    throw argError("ERR_INVALID_ARG_TYPE", `The "${name}" argument must be ${expected}`);
  }
}

const isObject = (v) => v !== null && typeof v === "object" && !Array.isArray(v);
const isBoolean = (v) => typeof v === "boolean";
const isString = (v) => typeof v === "string";

// The long name for `-x`: the option that declared it as its `short`, or the
// letter itself, so an undeclared `-x` is reported as `-x`.
function longFor(short, options) {
  for (const [name, config] of Object.entries(options)) {
    if (own(config, "short") === short) return name;
  }
  return short;
}

const isLoneShort = (arg) => arg.length === 2 && arg[0] === "-" && arg[1] !== "-";
const isLoneLong = (arg) => arg.length > 2 && arg.startsWith("--") && !arg.includes("=", 3);
const isLongWithValue = (arg) => arg.length > 2 && arg.startsWith("--") && arg.includes("=", 3);
const isShortCluster = (arg) => arg.length > 2 && arg[0] === "-" && arg[1] !== "-";
// A value that reads like an option: `--name --verbose` took `--verbose` as the
// name, which strict mode refuses as ambiguous.
const looksLikeOption = (value) => value != null && value.length > 1 && value[0] === "-";

function tokenize(argv, options) {
  const tokens = [];
  const rest = argv.slice();
  let index = -1;
  // Arguments expanded from a short group share the group's index.
  let grouped = 0;
  while (rest.length > 0) {
    const arg = rest.shift();
    const next = rest[0];
    if (grouped > 0) grouped--;
    else index++;

    if (arg === "--") {
      tokens.push({ kind: "option-terminator", index });
      for (const value of rest) tokens.push({ kind: "positional", index: ++index, value });
      break;
    }
    if (isLoneShort(arg)) {
      const name = longFor(arg[1], options);
      let value;
      let inlineValue;
      if (optionGet(options, name, "type") === "string" && next != null) {
        value = rest.shift();
        inlineValue = false;
      }
      tokens.push({ kind: "option", name, rawName: arg, index, value, inlineValue });
      if (value != null) index++;
      continue;
    }
    if (isShortCluster(arg)) {
      const name = longFor(arg[1], options);
      if (optionGet(options, name, "type") === "string") {
        // `-fFILE`: a string option with its value attached.
        tokens.push({
          kind: "option",
          name,
          rawName: `-${arg[1]}`,
          index,
          value: arg.slice(2),
          inlineValue: true,
        });
        continue;
      }
      // `-abc` is `-a -b -c`; a string option inside it takes the rest, so
      // `-abfFILE` is `-a -b -fFILE`.
      const expanded = [];
      for (let at = 1; at < arg.length; at++) {
        const short = arg[at];
        const type = optionGet(options, longFor(short, options), "type");
        if (type !== "string" || at === arg.length - 1) {
          expanded.push(`-${short}`);
        } else {
          expanded.push(`-${arg.slice(at)}`);
          break;
        }
      }
      rest.unshift(...expanded);
      grouped = expanded.length;
      continue;
    }
    if (isLoneLong(arg)) {
      const name = arg.slice(2);
      let value;
      let inlineValue;
      if (optionGet(options, name, "type") === "string" && next != null) {
        value = rest.shift();
        inlineValue = false;
      }
      tokens.push({ kind: "option", name, rawName: arg, index, value, inlineValue });
      if (value != null) index++;
      continue;
    }
    if (isLongWithValue(arg)) {
      const equals = arg.indexOf("=");
      const name = arg.slice(2, equals);
      tokens.push({
        kind: "option",
        name,
        rawName: `--${name}`,
        index,
        value: arg.slice(equals + 1),
        inlineValue: true,
      });
      continue;
    }
    tokens.push({ kind: "positional", index, value: arg });
  }
  return tokens;
}

function checkUsage(token, options, allowPositionals, allowNegative) {
  let name = token.name;
  if (!Object.prototype.hasOwnProperty.call(options, name)) {
    const negated = allowNegative && name.startsWith("no-") ? name.slice(3) : undefined;
    if (negated === undefined || optionGet(options, negated, "type") !== "boolean") {
      const hint = allowPositionals
        ? `. To specify a positional argument starting with a '-', place it at the end of the command after '--', as in '-- ${JSON.stringify(token.rawName)}'`
        : "";
      throw argError("ERR_PARSE_ARGS_UNKNOWN_OPTION", `Unknown option '${token.rawName}'${hint}`);
    }
    name = negated;
  }
  const short = optionGet(options, name, "short");
  const spelled = `${short ? `-${short}, ` : ""}--${name}`;
  const type = optionGet(options, name, "type");
  if (type === "string" && typeof token.value !== "string") {
    throw argError(
      "ERR_PARSE_ARGS_INVALID_OPTION_VALUE",
      `Option '${spelled} <value>' argument missing`,
    );
  }
  if (type === "boolean" && token.value != null) {
    throw argError(
      "ERR_PARSE_ARGS_INVALID_OPTION_VALUE",
      `Option '${spelled}' does not take an argument`,
    );
  }
  if (!token.inlineValue && looksLikeOption(token.value)) {
    const example = token.rawName.startsWith("--")
      ? `'${token.rawName}=-XYZ'`
      : `'--${token.name}=-XYZ' or '${token.rawName}-XYZ'`;
    throw argError(
      "ERR_PARSE_ARGS_INVALID_OPTION_VALUE",
      `Option '${token.rawName}' argument is ambiguous.\nDid you forget to specify the option argument for '${token.rawName}'?\nTo specify an option argument starting with a dash use ${example}.`,
    );
  }
}

function store(token, options, values, allowNegative) {
  let name = token.name;
  let value = token.value;
  // Never a key on `values`: it would be the one name that reaches a prototype.
  if (name === "__proto__") return;
  if (allowNegative && name.startsWith("no-") && value === undefined) {
    name = name.slice(3);
    token.name = name;
    value = false;
  }
  // Stored by what was written rather than by declared type, so a non-strict
  // parse keeps the program's intent for it to judge.
  const stored = value ?? true;
  if (optionGet(options, name, "multiple")) {
    if (values[name]) values[name].push(stored);
    else values[name] = [stored];
  } else {
    values[name] = stored;
  }
}

function parseArgs(config = {}) {
  checkType(config, "config", "an object", isObject);
  const argv = own(config, "args") ?? args;
  const strict = own(config, "strict") ?? true;
  const allowPositionals = own(config, "allowPositionals") ?? !strict;
  const wantTokens = own(config, "tokens") ?? false;
  const allowNegative = own(config, "allowNegative") ?? false;
  const options = own(config, "options") ?? { __proto__: null };

  checkType(argv, "args", "an array", Array.isArray);
  checkType(strict, "strict", "of type boolean", isBoolean);
  checkType(allowPositionals, "allowPositionals", "of type boolean", isBoolean);
  checkType(wantTokens, "tokens", "of type boolean", isBoolean);
  checkType(allowNegative, "allowNegative", "of type boolean", isBoolean);
  checkType(options, "options", "an object", isObject);
  for (const arg of argv) checkType(arg, "args", "an array of strings", isString);
  for (const [name, option] of Object.entries(options)) {
    checkType(option, `options.${name}`, "an object", isObject);
    const type = own(option, "type");
    if (type !== "string" && type !== "boolean") {
      throw argError(
        "ERR_INVALID_ARG_TYPE",
        `The "options.${name}.type" argument must be one of: 'string', 'boolean'. Received ${JSON.stringify(type) ?? String(type)}`,
      );
    }
    if (Object.prototype.hasOwnProperty.call(option, "short")) {
      const short = option.short;
      checkType(short, `options.${name}.short`, "of type string", isString);
      if (short.length !== 1) {
        throw argError(
          "ERR_INVALID_ARG_VALUE",
          `The property 'options.${name}.short' must be a single character. Received ${JSON.stringify(short)}`,
        );
      }
    }
    const multiple = own(option, "multiple");
    if (Object.prototype.hasOwnProperty.call(option, "multiple")) {
      checkType(multiple, `options.${name}.multiple`, "of type boolean", isBoolean);
    }
    const fallback = own(option, "default");
    if (fallback !== undefined) {
      const one = type === "string" ? isString : isBoolean;
      const ok = multiple ? (v) => Array.isArray(v) && v.every(one) : one;
      const expected = `${multiple ? "an array of " : "of type "}${type}${multiple ? "s" : ""}`;
      checkType(fallback, `options.${name}.default`, expected, ok);
    }
  }

  const tokens = tokenize(argv, options);
  const result = { values: { __proto__: null }, positionals: [] };
  if (wantTokens) result.tokens = tokens;
  for (const token of tokens) {
    if (token.kind === "option") {
      if (strict) checkUsage(token, options, allowPositionals, allowNegative);
      store(token, options, result.values, allowNegative);
    } else if (token.kind === "positional") {
      if (!allowPositionals) {
        throw argError(
          "ERR_PARSE_ARGS_UNEXPECTED_POSITIONAL",
          `Unexpected argument '${token.value}'. This command does not take positional arguments`,
        );
      }
      result.positionals.push(token.value);
    }
  }
  for (const [name, option] of Object.entries(options)) {
    const fallback = own(option, "default");
    if (name !== "__proto__" && fallback !== undefined && result.values[name] === undefined) {
      result.values[name] = fallback;
    }
  }
  return result;
}

export {
  env,
  args,
  parseArgs,
  stdin,
  platform,
  arch,
  cwd,
  exit,
  stdout,
  stderr,
  unmask,
  Secret,
  onSignal,
  offSignal,
  signals,
  permissions,
  memoryUsage,
  cpuTime,
  uptime,
  unrefTimer,
  refTimer,
};
export default {
  env,
  args,
  parseArgs,
  stdin,
  platform,
  arch,
  cwd,
  exit,
  stdout,
  stderr,
  unmask,
  Secret,
  onSignal,
  offSignal,
  signals,
  permissions,
  memoryUsage,
  cpuTime,
  uptime,
  unrefTimer,
  refTimer,
};
