// runtime:build — the bundler, callable from a program (esdev only).
//
// rolldown is already inside this binary; what was missing was a way for guest
// code to reach it. Without that, a framework's dev server has to import the
// bundler from npm — a napi addon, which this runtime does not load — and so
// has to be a Node program, which is the thing it was trying to stop being.
//
//   import { build } from "runtime:build";
//
//   const bundle = await build({
//     input: "app/main.jsx",
//     external: (id) => id.startsWith("/__route/"),
//     resolve: { alias: { "@": "./src" }, extensions: [".js", ".jsx"] },
//     define: { "process.env.NODE_ENV": '"development"' },
//     plugins: [mdx(), css()],
//   });
//
//   const { output, watchFiles } = await bundle.generate({
//     format: "esm",
//     codeSplitting: false,
//   });
//   serve(output[0].code);          // never written to disk
//
// The API is rollup's, deliberately: it is what every plugin ever written
// expects, and a fourth spelling of the same idea would buy nothing.
//
// Two things are worth knowing beyond that.
//
// **`watchFiles` is the point of the return value.** It is every file the build
// read, plus every file a plugin declared with `this.addWatchFile()` — which is
// what lets a consumer drop the three cached chunks that used a changed file
// and keep the other thirty-seven. Pair it with `runtime:watch`.
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

// The hooks this bridge carries. A plugin may define others — they are simply
// not called, rather than refused, so a plugin written for rollup still works
// minus whatever is not here.
const HOOKS = ["buildStart", "resolveId", "load", "transform", "buildEnd"];

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
  // `external` is a bare function, not a plugin object.
  if (call.hook === "external") return target(...call.args);
  const hook = target[call.hook];
  if (typeof hook !== "function") return null;
  return hook.apply(context(call.id), call.args);
}

// A hook's `this`. Every method is bound to the call it belongs to, and stops
// working when that call returns — the context is the bundler's, and reaching
// into it from a hook that has already finished would be reaching into a build
// that may no longer exist.
function context(id) {
  return {
    // The bundler's own resolver, mid-hook. `null` when nothing resolves.
    resolve(source, importer, options) {
      return ops.build_resolve(id, String(source), importer ?? null, options ?? null);
    },
    // A dependency the graph could not have discovered — frontmatter, a
    // `_meta.js` a virtual module was built from. This is the input to
    // fine-grained invalidation.
    addWatchFile(file) {
      ops.build_watch_file(id, String(file));
      return undefined;
    },
    emitFile(file) {
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
    // Rollup's `this.error` throws, and so does this one: the plugin is saying
    // the build cannot continue, and returning would be pretending otherwise.
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

// A hook's return value, in the shape the host reads. Only `load` and
// `transform` need anything doing: a source map is an object here and JSON on
// the way across, because that is what every tool that makes one produces and
// re-encoding it field by field would be the same bytes with more ways to go
// wrong.
function normalize(hook, value) {
  if (hook !== "load" && hook !== "transform") return value ?? null;
  if (value === null || value === undefined) return null;
  if (typeof value === "string") return value;
  const map = value.map;
  return {
    code: value.code,
    map: map == null ? null : typeof map === "string" ? map : JSON.stringify(map),
    ...(value.moduleType == null ? {} : { moduleType: String(value.moduleType) }),
  };
}

// Hands the host a plugin as { id, name, hooks } — the handle, and which hooks
// it actually has. Declaring the hooks is not bookkeeping: a plugin listed as
// having `transform` puts *every module in the graph* through a round trip to
// this isolate, so a hook it does not implement is a cost paid per module for
// nothing.
function describe(plugin, index) {
  if (plugin === null || typeof plugin !== "object") {
    throw new TypeError("build: each plugin must be an object");
  }
  return {
    id: register(plugin),
    name: String(plugin.name ?? `plugin-${index}`),
    hooks: HOOKS.filter((hook) => typeof plugin[hook] === "function"),
  };
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
  constructor(ready) {
    this._ready = ready; // Promise<handle>
    this._closed = false;
    // A build that could not be created must not also end the process as an
    // unhandled rejection; `generate()` still rejects.
    this._ready.catch(() => {});
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
    ops.build_close(await this._ready);
  }

  async _run(op, output) {
    if (this._closed) throw new Error("this build is closed");
    const handle = await this._ready;
    const result = await ops[op](handle, output ?? null);
    this.watchFiles = result.watchFiles;
    this.warnings = result.warnings;
    return result;
  }
}

// Starts a build. The bundler runs on a thread of its own; this resolves once
// it has accepted the options, and `generate()` is what does the work.
async function build(options) {
  const prepared = prepare(options);
  // Started before the first hook can possibly fire.
  pump();
  return new Bundle(ops.build_create(prepared));
}

export { build, Bundle };
export default { build };
