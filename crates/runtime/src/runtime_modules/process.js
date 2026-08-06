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
// op, not the import (DECISIONS D26/D38). `env` and `args` are the only bindings
// here whose values come from an `Env`-gated op, so both seed themselves on
// *first access* rather than at module evaluation. Under `--deny-env` this
// module still imports (and `exit`, `onSignal`, `permissions` still work);
// touching `env`/`args` is what throws.
function seeded(target, fill) {
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
    set: (t, k, v, r) => (seed(), Reflect.set(t, k, v, r)),
    has: (t, k) => (seed(), Reflect.has(t, k)),
    deleteProperty: (t, k) => (seed(), Reflect.deleteProperty(t, k)),
    ownKeys: (t) => (seed(), Reflect.ownKeys(t)),
    getOwnPropertyDescriptor: (t, k) => (
      seed(), Reflect.getOwnPropertyDescriptor(t, k)
    ),
    defineProperty: (t, k, d) => (seed(), Reflect.defineProperty(t, k, d)),
    // Without this, an `Object.freeze(env)` before any read would lock an empty
    // target and the later seeding would silently fail.
    preventExtensions: (t) => (seed(), Reflect.preventExtensions(t)),
  });
}

// `env`: a mutable in-process object seeded from the host snapshot. Reads,
// writes, and deletes work in-process; they do not (yet) propagate to the host
// process or future child processes. Secret-keyed values are wrapped (above).
const env = seeded({}, (target) => {
  for (const [key, value] of ops.process_env()) {
    target[key] = SECRET_KEY.test(key) ? new Secret(value) : value;
  }
});

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

// The denial vocabulary, identical to the --deny-<name> flag suffixes. The
// authoritative list is Rust-side (Capability::HOST_FACING); this copy exists
// only to reject typos in has() rather than answering them.
const PERMISSIONS = Object.freeze([
  "read",
  "write",
  "imports",
  "net",
  "listen",
  "env",
  "run",
  "signals",
  "workers",
]);

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

export {
  env,
  args,
  platform,
  arch,
  cwd,
  exit,
  unmask,
  Secret,
  onSignal,
  offSignal,
  signals,
  permissions,
};
export default {
  env,
  args,
  platform,
  arch,
  cwd,
  exit,
  unmask,
  Secret,
  onSignal,
  offSignal,
  signals,
  permissions,
};
