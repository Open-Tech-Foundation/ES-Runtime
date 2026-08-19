// runtime:build — the bundler, callable from a program (esdev only).
//
// rolldown is already inside this binary; what was missing was a way for guest
// code to reach it. Without that, a framework's dev server has to import the
// bundler from npm — a napi addon, which this runtime does not load — and so
// has to be a Node program, which is the thing it was trying to stop being.
//
//   import { build } from "runtime:build";
//
//   const mdx = {
//     name: "mdx",
//     transform: {
//       filter: { id: /\.mdx$/ },
//       handler(code, id, ctx) {
//         const { js, meta } = compile(code, id);
//         return { code: js, type: "jsx", dependsOn: [meta] };
//       },
//     },
//   };
//
//   const bundle = await build({
//     input: "app/main.jsx",
//     external: (id) => id.startsWith("/__route/"),
//     resolve: { alias: { "@": "./src" }, extensions: [".js", ".jsx"] },
//     define: { "process.env.NODE_ENV": '"development"' },
//     plugins: [mdx],
//   });
//
//   const { output, watchFiles } = await bundle.generate({
//     format: "esm",
//     codeSplitting: false,
//   });
//   serve(output[0].code);          // never written to disk
//
// The plugin system is **ours**, not the bundler's passed through. Three things
// in it are not inherited from rollup, and each earns its place here for a
// reason that does not apply to a bundler running in one process:
//
//   * **A filter is declarative**, and matched on the host's side. In rollup a
//     hook that returns null costs a function call; here it costs a round trip
//     into this isolate, so an unfiltered `transform` is one crossing per
//     module in the graph — four hundred of them to reach one .mdx file.
//   * **Dependencies are returned**, not declared by calling something. A
//     `dependsOn` you forget is a build that serves stale output, which is the
//     failure hardest to notice; a field of the value you return is not
//     forgettable in the same way.
//   * **A virtual module says `virtual: true`** rather than being signalled by
//     a NUL byte glued to the front of its id.
//
// And the context is the **last argument**, not `this`, so an arrow-function
// hook cannot silently lose it.
//
// Two more things worth knowing.
//
// **`watchFiles` is the point of the return value.** It is every file the build
// read, plus every file a plugin declared — which is what lets a consumer drop
// the three cached chunks that used a changed file and keep the other
// thirty-seven. Pair it with `runtime:watch`.
//
// **Hooks run in this isolate, not in the bundler's threads.** The bundler
// works in parallel on threads of its own and posts each hook call here; the
// pump below answers them. Several can be in flight at once — the pump does not
// wait for one hook before accepting the next — so a slow plugin holds up its
// own module rather than the build.

const ops = globalThis.__ops;

// Every plugin object and `external` predicate the guest has handed over, by
// the handle the host knows it as. The host never sees a function: it sees a
// number, and asks for it back by that number.
const registry = new Map();
let nextHandle = 1;

function register(value) {
  const handle = nextHandle++;
  registry.set(handle, value);
  return handle;
}

// The hooks the contract carries. Five, against rollup's twenty-odd: each one
// is a promise some future bundler behind this has to keep, so the list is
// short on purpose and grows only when something cannot be written without it.
const HOOKS = ["start", "resolve", "load", "transform", "end"];

// The pump. One for the whole program: hook calls carry the handle of the
// plugin they are for, so there is nothing to keep separate.
let pumping = false;

async function pump() {
  if (pumping) return;
  pumping = true;
  try {
    for (;;) {
      const call = await ops.build_hook();
      if (call === null) return;
      // Deliberately not awaited: the next call is accepted while this one
      // runs, which is what keeps the bundler's parallelism alive across the
      // crossing into this isolate.
      dispatch(call);
    }
  } finally {
    pumping = false;
  }
}

async function dispatch(call) {
  try {
    const value = await invoke(call);
    ops.build_hook_reply(call.id, true, normalize(call.hook, value));
  } catch (err) {
    // The build fails with what the plugin threw, stack and all: a plugin
    // error that arrived as "build failed" would be the worst possible
    // outcome of putting plugins in a different thread from the bundler.
    ops.build_hook_reply(call.id, false, String(err?.stack ?? err));
  }
}

function invoke(call) {
  const target = registry.get(call.plugin);
  if (target === undefined) return null;
  // `external` is a bare predicate, not a plugin.
  if (call.hook === "external") return target(...call.args);
  const handler = target[call.hook];
  if (typeof handler !== "function") return null;
  // The context is the **last argument**, not `this`. `this` is left undefined
  // deliberately: rollup's context-as-`this` is silently lost by an arrow
  // function, and a hook whose `this.resolve` is undefined fails somewhere far
  // from the arrow that caused it.
  //
  // Every hook is *data first, context last*, with nothing positional in
  // between — anything a particular hook needs to say (`isEntry`, on a
  // resolve) rides on the context instead of shifting the signature.
  return handler(...call.args, context(call.id, call.meta));
}

// A hook's context: the bundler's own, for exactly as long as that hook runs.
// Reaching into it afterwards throws, because by then it may name a build that
// no longer exists.
function context(id, meta) {
  return {
    ...meta,
    // The bundler's own resolver, mid-hook. `null` when nothing resolves.
    resolve(source, importer, options) {
      return ops.build_resolve(id, String(source), importer ?? null, options ?? null);
    },
    // Adds a chunk or an asset to a build that is already running.
    emit(file) {
      return ops.build_emit(id, file);
    },
    warn(log) {
      ops.build_log(id, "warn", message(log));
      return undefined;
    },
    info(log) {
      ops.build_log(id, "info", message(log));
      return undefined;
    },
    debug(log) {
      ops.build_log(id, "debug", message(log));
      return undefined;
    },
    // Fails the build. Throws — it does not return, because the plugin is
    // saying the build cannot continue and returning would pretend otherwise.
    error(log) {
      throw log instanceof Error ? log : new Error(message(log));
    },
  };
}

function message(log) {
  if (log === null || log === undefined) return "";
  if (typeof log === "string") return log;
  if (log instanceof Error) return log.message;
  return String(log.message ?? log);
}

// A hook's return value, in the shape the host reads.
//
// `null` and `undefined` mean "not mine" — the one convention worth keeping,
// because a hook has to be able to decline. Everything else must be the object
// form: a bare string of code is rollup's shorthand, and accepting it would
// make this a superset of somebody else's design rather than a contract of its
// own. A source map is stringified here because JSON is the form every tool
// that makes one already produces.
function normalize(hook, value) {
  if (value === null || value === undefined) return null;
  if (hook === "resolve") {
    if (typeof value === "string") {
      throw new TypeError(
        `resolve must return an object or null — return { id: ${JSON.stringify(value)} }`,
      );
    }
    return value;
  }
  if (hook !== "load" && hook !== "transform") return value;
  if (typeof value === "string") {
    throw new TypeError(`${hook} must return an object or null — return { code } instead`);
  }
  const map = value.map;
  return {
    ...value,
    map: map == null ? null : typeof map === "string" ? map : JSON.stringify(map),
  };
}

// --- reading a plugin declaration -------------------------------------------

// Levenshtein distance, for naming the hook a typo was nearly.
function distance(a, b) {
  let prev = Array.from({ length: b.length + 1 }, (_, i) => i);
  for (let i = 0; i < a.length; i++) {
    const row = [i + 1];
    for (let j = 0; j < b.length; j++) {
      row[j + 1] = Math.min(
        prev[j] + (a[i] === b[j] ? 0 : 1),
        prev[j + 1] + 1,
        row[j] + 1,
      );
    }
    prev = row;
  }
  return prev[b.length];
}

function unknownHook(name, given) {
  const near = HOOKS.find((hook) => distance(given.toLowerCase(), hook) <= 2);
  throw new TypeError(
    near
      ? `${name}: unknown hook "${given}". Did you mean "${near}"?`
      : `${name}: unknown hook "${given}". The hooks are: ${HOOKS.join(", ")}`,
  );
}

// A `RegExp`, as the two parts the host can rebuild it from. The host compiles
// it with Rust's `regex`, whose syntax is smaller than JavaScript's: `\0` and
// `\/` are translated, and anything left that will not compile — a
// backreference, a lookaround — is **refused where it was written**. It used to
// stop filtering instead, which meant `/\0virtual/` (rollup's virtual-id
// convention, and so the first filter a ported plugin writes) matched every
// module in the graph and the plugin's `load` claimed the entry.
function pattern(value) {
  if (typeof value === "string") return value;
  if (value instanceof RegExp) return { source: value.source, flags: value.flags };
  throw new TypeError("a filter pattern must be a string or a RegExp");
}

function patterns(value) {
  if (value === undefined || value === null) return undefined;
  return Array.isArray(value) ? value.map(pattern) : pattern(value);
}

function filterOf(name, hook, filter) {
  if (filter === undefined || filter === null) return undefined;
  if (typeof filter !== "object") {
    throw new TypeError(`${name}.${hook}: filter must be an object`);
  }
  const out = {};
  const id = patterns(filter.id);
  const code = patterns(filter.code);
  if (id !== undefined) out.id = id;
  if (code !== undefined) out.code = code;
  return out;
}

// One hook, as declared. **There is one form**: an object carrying a handler,
// optionally a filter and an order. A bare function is rollup's shorthand and
// is refused — accepting it would make the filter, the order and the context
// argument optional extras on somebody else's design.
function hookOf(name, hook, declared) {
  if (typeof declared === "function") {
    throw new TypeError(
      `${name}.${hook}: a hook is an object, not a function — ` +
        `write { handler(...) {} }, optionally with filter and order`,
    );
  }
  if (declared === null || typeof declared !== "object") {
    throw new TypeError(`${name}.${hook}: a hook must be an object with a handler`);
  }
  if (typeof declared.handler !== "function") {
    throw new TypeError(`${name}.${hook}: handler must be a function`);
  }
  const spec = {};
  const filter = filterOf(name, hook, declared.filter);
  if (filter !== undefined) spec.filter = filter;
  if (declared.order !== undefined) spec.order = String(declared.order);
  return spec;
}

// Hands the host a plugin as { id, name, hooks } — the handle, and what each
// hook declared. The *filter* is the part that matters most: it is matched on
// the host's side, so an unfiltered `transform` is one crossing into this
// isolate per module in the graph.
function describe(plugin, index) {
  if (plugin === null || typeof plugin !== "object") {
    throw new TypeError("build: each plugin must be an object");
  }
  const name = String(plugin.name ?? `plugin-${index}`);
  const handlers = {};
  const hooks = {};
  for (const key of Object.keys(plugin)) {
    if (key === "name") continue;
    if (!HOOKS.includes(key)) unknownHook(name, key);
    hooks[key] = hookOf(name, key, plugin[key]);
    handlers[key] = plugin[key].handler;
  }
  return { id: register(handlers), name, hooks };
}

// The options, with every function replaced by the handle the host calls back
// through. Nothing else is rewritten.
function prepare(options) {
  if (options === null || typeof options !== "object") {
    throw new TypeError(`build: options must be an object, got ${typeof options}`);
  }
  const prepared = { ...options };
  if (typeof options.external === "function") {
    prepared.external = register(options.external);
  }
  const plugins = options.plugins ?? [];
  if (!Array.isArray(plugins)) throw new TypeError("build: plugins must be an array");
  prepared.plugins = plugins.filter((p) => p != null).map(describe);
  return prepared;
}

// A build: the options, and whatever the last generate() produced.
class Bundle {
  constructor(handle) {
    this._handle = handle;
    this._closed = false;
    // Every file the last build read. Empty until then — the scan is what
    // discovers them, and there has not been one yet.
    this.watchFiles = [];
    this.warnings = [];
  }

  // Builds, and returns the chunks and assets in memory. Nothing is written.
  async generate(output) {
    return this._run("build_generate", output);
  }

  // The same build, landed on disk under `dir`. Needs FileWrite as well.
  async write(output) {
    return this._run("build_write", output);
  }

  // Releases the build. A bundle that is not closed holds only its options, so
  // this is tidiness rather than a leak — but a long-running dev server makes
  // thousands of them.
  async close() {
    if (this._closed) return;
    this._closed = true;
    ops.build_close(this._handle);
  }

  async _run(op, output) {
    if (this._closed) throw new Error("this build is closed");
    const result = await ops[op](this._handle, output ?? null);
    this.watchFiles = result.watchFiles;
    this.warnings = result.warnings;
    return result;
  }
}

// Starts a build. The bundler runs on a thread of its own; this resolves once
// it has accepted the options, and `generate()` is what does the work.
//
// The options — and every plugin declaration in them — are checked *here*,
// which is why this awaits rather than handing back a bundle wrapped around a
// pending handle. A plugin that cannot be read is a build that cannot start,
// and the rejection belongs at the line that wrote the declaration rather than
// at the `generate()` three lines further down.
async function build(options) {
  const prepared = prepare(options);
  // Started before the first hook can possibly fire.
  pump();
  return new Bundle(await ops.build_create(prepared));
}

export { build, Bundle };
export default { build };
