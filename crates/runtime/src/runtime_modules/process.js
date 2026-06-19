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

export { env, args, platform, arch, cwd, exit, unmask, Secret };
export default { env, args, platform, arch, cwd, exit, unmask, Secret };
