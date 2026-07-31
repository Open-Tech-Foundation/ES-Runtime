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

// `env`: a mutable in-process object seeded from the host snapshot. Reads,
// writes, and deletes work in-process; they do not (yet) propagate to the host
// process or future child processes. Secret-keyed values are wrapped (above).
const env = {};
for (const [key, value] of ops.process_env()) {
  env[key] = SECRET_KEY.test(key) ? new Secret(value) : value;
}

// `args`: the program arguments after the runtime binary and the script/-e code.
const args = Object.freeze(ops.process_args());

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

export { env, args, platform, arch, cwd, exit, unmask, Secret, onSignal, offSignal, signals };
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
};
