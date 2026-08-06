// Runs a subset of the Web Platform Tests under esrun:
//
//     esrun wpt/run.js                       # every mode, summary only
//     esrun wpt/run.js --mode=worker -v      # only inside real workers, verbose
//     esrun wpt/run.js --filter=Channel      # substring match on the test path
//     esrun wpt/run.js --update-expectations # re-record expectations.json
//
// The curated `crates/runtime/conformance` suite states spec behaviour in our
// own words. This runs the upstream tests unmodified, which is the only way to
// find the deviations we did not think to write down — and, unlike the curated
// suite, it runs them *inside a worker*, which is where the worker surface
// actually lives.
//
// Scope is `.any.js` and `.worker.js` only. WPT's `.html` tests need a document
// and a server that does substitution; this is a server-side runtime, so they
// are out of scope rather than failing (SPEC §14).
import { file, write, remove, Glob } from "runtime:fs";
import { args } from "runtime:process";
import { fileScope, subtestScope } from "./scope.js";

const UPSTREAM = new URL("./upstream/", import.meta.url);
const EXPECTATIONS = new URL("./expectations.json", import.meta.url);

// Directories scanned for tests. Everything a worker touches: the worker itself,
// HTML messaging (ports, channels, BroadcastChannel), and the serialization
// algorithm the two share.
const ROOTS = ["workers", "webmessaging", "html/webappapis/structured-clone"];

// WPT test-status codes (testharness.js `Test.statuses` / `harness_statuses`).
const TEST_STATUS = ["PASS", "FAIL", "TIMEOUT", "NOTRUN", "PRECONDITION_FAILED"];
const HARNESS_STATUS = ["OK", "ERROR", "TIMEOUT", "PRECONDITION_FAILED"];

// What a worker spawned for a test may do. A conformance run should be limited
// by the runtime, not by the sandbox, so this is everything a worker can hold
// except the three that reach outside the process.
const WORKER_PERMISSIONS = ["read", "write", "imports", "net", "listen", "workers", "env"];

const DEFAULT_TIMEOUT_MS = 10_000;
const LONG_TIMEOUT_MS = 60_000;

// ---- arguments --------------------------------------------------------------

const flags = {
  mode: "both",
  filter: "",
  verbose: false,
  update: false,
  keep: false,
  json: "",
  timeout: 0,
};
for (const arg of args) {
  // `esrun` claims short flags before the script name, so a run that wants
  // `--verbose` passes `esrun wpt/run.js -- --verbose`.
  if (arg === "--") continue;
  const [key, value = ""] = arg.replace(/^--?/, "").split("=");
  if (key === "mode") flags.mode = value;
  else if (key === "filter" || key === "f") flags.filter = value;
  else if (key === "verbose" || key === "v") flags.verbose = true;
  else if (key === "update-expectations") flags.update = true;
  else if (key === "keep") flags.keep = true;
  else if (key === "json") flags.json = value;
  else if (key === "timeout") flags.timeout = Number(value);
  else throw new Error(`unknown argument: ${arg}`);
}

// ---- discovery --------------------------------------------------------------

// `.any.js` expands to one test per global scope it names; `worker` is itself
// shorthand for the three worker scopes (tools/manifest/sourcefile.py).
const GLOBAL_LONGHAND = {
  worker: ["dedicatedworker", "sharedworker", "serviceworker"],
};
const DEFAULT_GLOBALS = ["window", "dedicatedworker"];

// Helper scripts, not tests.
const HELPER_DIR = /\/(resources|support)\//;

function parseMeta(source) {
  const meta = { globals: null, scripts: [], title: "", timeout: "", variants: [] };
  for (const line of source.split("\n")) {
    const match = /^\/\/\s*META:\s*([a-z]+)=(.*)$/.exec(line.trim());
    if (!match) {
      // META lines are a header: the first line that is not one ends them.
      if (line.trim().startsWith("//") || line.trim() === "") continue;
      break;
    }
    const [, key, value] = match;
    if (key === "global") meta.globals = value.split(",").map((s) => s.trim());
    else if (key === "script") meta.scripts.push(value.trim());
    else if (key === "title") meta.title = value.trim();
    else if (key === "timeout") meta.timeout = value.trim();
    else if (key === "variant") meta.variants.push(value.trim());
  }
  return meta;
}

function modesFor(path, meta) {
  // A `.worker.js` file is a dedicated worker's body, and nothing else.
  if (path.endsWith(".worker.js")) return { modes: ["worker"], skip: "" };

  const globals = new Set();
  for (const name of meta.globals ?? DEFAULT_GLOBALS) {
    for (const expanded of GLOBAL_LONGHAND[name] ?? [name]) globals.add(expanded);
  }
  const modes = [];
  // No window here — the agent that drives the process is the closest thing this
  // runtime has to one, and it is what a `window` test is really asking for: the
  // scope that is not a worker.
  if (globals.has("window")) modes.push("main");
  if (globals.has("dedicatedworker")) modes.push("worker");

  if (modes.length === 0) {
    const only = [...globals].join(",");
    return { modes: [], skip: `no scope this runtime has (global=${only})` };
  }
  return { modes, skip: "" };
}

async function discover() {
  const found = [];
  for (const root of ROOTS) {
    const dir = new URL(`${root}/`, UPSTREAM).pathname;
    for await (const name of new Glob("**/*.js").scan({ cwd: dir })) {
      const relative = `${root}/${name}`;
      if (!name.endsWith(".any.js") && !name.endsWith(".worker.js")) continue;
      // Generated by a previous run that did not get to clean up.
      if (name.includes(".__wpt_")) continue;
      if (HELPER_DIR.test(`/${name}`)) continue;
      // `.sub.` files are rewritten by WPT's server before they are served, and
      // `.window.js` needs a document by definition.
      if (name.includes(".sub.")) continue;
      if (flags.filter && !relative.includes(flags.filter)) continue;
      found.push(relative);
    }
  }
  return found.sort();
}

// ---- bundling ---------------------------------------------------------------

const harnessSource = await file(new URL("resources/testharness.js", UPSTREAM).pathname).text();

const sourceCache = new Map();
async function read(path) {
  if (!sourceCache.has(path)) {
    sourceCache.set(path, await file(new URL(path, UPSTREAM).pathname).text());
  }
  return sourceCache.get(path);
}

// A `// META: script=` path is absolute against the WPT root, or relative to the
// test file.
function resolveScript(spec, testPath) {
  if (spec.startsWith("/")) return spec.slice(1);
  const dir = testPath.slice(0, testPath.lastIndexOf("/") + 1);
  return new URL(spec, `file:///${dir}`).pathname.slice(1);
}

// One test becomes one generated module: the harness, a result sink, whatever
// `META: script=` asked for, then the test itself. The same bundle is imported
// on this agent or handed to `new Worker`, so a difference between the two
// results is a difference in the runtime, not in how the test was assembled.
//
// It is written next to the original file so that relative URLs inside the test
// — `new Worker("support/WorkerBasic.js")` — resolve where the test expects.
//
// The payload runs through **indirect eval**, not as the module body. A WPT test
// is a classic script: sloppy mode, `var` and function declarations that land on
// the global, and helpers that assign an undeclared name outright
// (`structuredCloneBatteryOfTests = []` in the structured-clone battery). A
// module body is strict and its scope is its own, so running it there fails
// tests over a difference in how they were loaded. `(0, eval)` is the closest
// thing a module-only runtime has to a classic script, and it is what
// crates/runtime/conformance/run.js already does.
function prologue(meta) {
  return `globalThis.META_TITLE = ${JSON.stringify(meta.title)};
globalThis.importScripts = function (...urls) {
  for (const url of urls) {
    // The harness is already in this bundle; anything else would need classic
    // script evaluation, which this runtime does not have (SPEC §8).
    if (!/\\/resources\\/testharness\\.js$/.test(String(url))) {
      throw new Error("importScripts() is not supported: " + url);
    }
  }
};`;
}

const COLLECTOR = `
(function () {
  // On this agent the runner installs a sink directly; in a worker the only way
  // back is the message channel it was started with.
  var send = typeof globalThis.__wptSink === "function"
    ? globalThis.__wptSink
    : function (report) { globalThis.postMessage({ __wpt: report }); };
  add_completion_callback(function (tests, status) {
    send({
      harness: { status: status.status, message: status.message || "" },
      tests: tests.map(function (t) {
        return { name: t.name, status: t.status, message: t.message || "" };
      }),
    });
  });
})();`;

// WPT serves the checkout at "/", so a test may name a helper as
// "/workers/support/x.js". There is no server here, so the same mapping is done
// textually, on string literals only — the alternative is failing a test over
// how it addressed a file rather than what it asserts.
const ROOTED = new RegExp(`(["'\`])/(${[...ROOTS.map((r) => r.split("/")[0]), "common", "resources"].join("|")})/`, "g");
function mapRootedPaths(source) {
  return source.replace(ROOTED, (_, quote, dir) => `${quote}${UPSTREAM.pathname}${dir}/`);
}

async function bundle(testPath, meta, mode) {
  const parts = [prologue(meta), harnessSource, COLLECTOR];
  for (const spec of meta.scripts) parts.push(mapRootedPaths(await read(resolveScript(spec, testPath))));
  parts.push(mapRootedPaths(await read(testPath)));

  const module = `// Generated by wpt/run.js from ${testPath} (${mode}). Deleted when the run finishes.
(0, eval)(${JSON.stringify(parts.join("\n;\n"))});
`;
  const bundlePath = testPath.replace(/\.js$/, `.__wpt_${mode}.js`);
  await write(new URL(bundlePath, UPSTREAM).pathname, module);
  return bundlePath;
}

// ---- running ----------------------------------------------------------------

function deadline(ms) {
  let timer;
  const promise = new Promise((resolve) => {
    timer = setTimeout(() => resolve(null), ms);
  });
  return { promise, cancel: () => clearTimeout(timer) };
}

async function runOnMainAgent(bundleUrl, timeoutMs) {
  let settle;
  const reported = new Promise((resolve) => (settle = resolve));
  globalThis.__wptSink = settle;
  // The deadline is armed *after* the import, not around it. A dynamic import()
  // resolves only when the event loop next wakes for some other reason, so a
  // pending timer delays it by that timer's full duration — arming a 10 s
  // deadline first makes every import take 10 s and every test time out. See
  // wpt/README.md.
  try {
    await import(bundleUrl);
  } catch (error) {
    return { harness: { status: 1, message: `${error?.message ?? error}` }, tests: [] };
  }
  const timer = deadline(timeoutMs);
  const report = await Promise.race([reported, timer.promise]);
  timer.cancel();
  delete globalThis.__wptSink;
  return report ?? { harness: { status: 2, message: "no result before the deadline" }, tests: [] };
}

async function runInWorker(bundleUrl, timeoutMs) {
  let settle;
  const reported = new Promise((resolve) => (settle = resolve));
  const worker = new Worker(bundleUrl, { permissions: WORKER_PERMISSIONS });
  worker.onmessage = (event) => {
    if (event.data && event.data.__wpt) settle(event.data.__wpt);
  };
  worker.onerror = (event) => {
    // Taking responsibility: an uncaught error in the worker is this test's
    // result, not something to report a second time on the way out.
    event.preventDefault();
    settle({ harness: { status: 1, message: event.message }, tests: [] });
  };
  const timer = deadline(timeoutMs);
  const report = await Promise.race([reported, timer.promise]);
  timer.cancel();
  worker.terminate();
  return report ?? { harness: { status: 2, message: "no result before the deadline" }, tests: [] };
}

// ---- reporting --------------------------------------------------------------

// Subtest tallies. `SKIP` is a subtest ruled out by wpt/scope.js — a browser
// test, not a deviation — and is excluded from the pass rate rather than
// counted against it.
const totals = { PASS: 0, FAIL: 0, TIMEOUT: 0, NOTRUN: 0, PRECONDITION_FAILED: 0, SKIP: 0 };
const runs = { OK: 0, ERROR: 0, TIMEOUT: 0, PRECONDITION_FAILED: 0 };
const harnessErrors = [];
const skipped = [];
const results = {};
const generated = [];

const discovered = await discover();
const tests = [];
for (const testPath of discovered) {
  const reason = fileScope(testPath);
  if (reason) skipped.push(`${testPath} — ${reason}`);
  else tests.push(testPath);
}
console.log(`${discovered.length} test files under ${ROOTS.join(", ")}\n`);

for (const testPath of tests) {
  const source = await read(testPath);
  const meta = parseMeta(source);
  const { modes, skip } = modesFor(testPath, meta);
  if (skip) {
    skipped.push(`${testPath} — ${skip}`);
    continue;
  }
  if (meta.variants.length > 0) {
    skipped.push(`${testPath} — META: variant is not supported by this runner`);
    continue;
  }

  const timeoutMs =
    flags.timeout || (meta.timeout === "long" ? LONG_TIMEOUT_MS : DEFAULT_TIMEOUT_MS);
  for (const mode of modes) {
    if (flags.mode !== "both" && flags.mode !== mode) continue;

    const bundlePath = await bundle(testPath, meta, mode);
    generated.push(bundlePath);
    const bundleUrl = new URL(bundlePath, UPSTREAM).href;

    const report =
      mode === "main"
        ? await runOnMainAgent(bundleUrl, timeoutMs)
        : await runInWorker(bundleUrl, timeoutMs);

    const key = `${testPath}:${mode}`;
    const subtests = {};
    const notable = [];
    let passed = 0;
    let runnable = 0;
    for (const t of report.tests) {
      const outOfScope = subtestScope(testPath, t.name);
      const status = outOfScope ? "SKIP" : (TEST_STATUS[t.status] ?? "FAIL");
      totals[status] += 1;
      subtests[t.name] = status;
      if (status === "SKIP") continue;
      runnable += 1;
      if (status === "PASS") passed += 1;
      else notable.push(`  ${status} ${t.name}${t.message ? ` — ${t.message}` : ""}`);
    }
    const harness = HARNESS_STATUS[report.harness.status] ?? "ERROR";
    runs[harness] += 1;
    if (harness !== "OK") {
      harnessErrors.push(`${key} — ${harness}: ${report.harness.message}`);
    }
    results[key] = { harness, subtests };

    const label = harness === "OK" ? `${passed}/${runnable}` : harness;
    console.log(`${label.padStart(9)}  ${key}`);
    if (flags.verbose) for (const line of notable) console.log(line);
  }
}

if (!flags.keep) {
  for (const path of generated) await remove(new URL(path, UPSTREAM).pathname);
}

const subtestTotal = Object.values(totals).reduce((a, b) => a + b, 0);
const runnable = subtestTotal - totals.SKIP;
const failed = runnable - totals.PASS;
const runTotal = Object.values(runs).reduce((a, b) => a + b, 0);
const pct = (n, of) => (of === 0 ? "—" : `${((100 * n) / of).toFixed(1)}%`);

console.log(`
                total   runnable   skipped   errored   timeout   passed   failed
  files    ${String(discovered.length).padStart(9)}${String(discovered.length - skipped.length).padStart(11)}${String(skipped.length).padStart(10)}
  runs     ${String(runTotal).padStart(9)}${String(runs.OK).padStart(11)}${"—".padStart(10)}${String(runs.ERROR).padStart(10)}${String(runs.TIMEOUT).padStart(10)}
  subtests ${String(subtestTotal).padStart(9)}${String(runnable).padStart(11)}${String(totals.SKIP).padStart(10)}${"—".padStart(10)}${"—".padStart(10)}${String(totals.PASS).padStart(9)}${String(failed).padStart(9)}

  ${totals.PASS}/${runnable} runnable subtests passing (${pct(totals.PASS, runnable)}) — \
${totals.SKIP} skipped as browser-only, see wpt/scope.js`);

if (harnessErrors.length > 0) {
  console.log(`\n${harnessErrors.length} run(s) failed before their tests could report:`);
  for (const line of harnessErrors) console.log(`  ${line}`);
}
if (skipped.length > 0) {
  console.log(`\n${skipped.length} file(s) skipped:`);
  if (flags.verbose) for (const line of skipped) console.log(`  ${line}`);
}

if (flags.json) await write(flags.json, JSON.stringify({ totals, results, skipped }, null, 2));

// ---- expectations -----------------------------------------------------------

// A recorded expectation is a floor: a subtest that used to pass and now does
// not fails the run. A subtest that starts passing is reported too — the record
// is stale, and a fix should land with its expectation updated.
//
// Nothing here calls `exit()`. It does not work: `exit()` from a module that
// used top-level `await` hangs the process unless it is the very last statement
// — see wpt/README.md. The runner ends by falling off the end, or by throwing.
const regressions = [];

if (flags.update) {
  await write(EXPECTATIONS.pathname, `${JSON.stringify(results, null, 2)}\n`);
  console.log(`\nrecorded ${Object.keys(results).length} runs to wpt/expectations.json`);
} else if (!(await file(EXPECTATIONS.pathname).exists())) {
  console.log("\nno wpt/expectations.json yet — run with --update-expectations to record one");
} else {
  const expected = await file(EXPECTATIONS.pathname).json();
  const progressions = [];
  for (const [key, run] of Object.entries(results)) {
    const before = expected[key];
    if (!before) continue;
    for (const [name, status] of Object.entries(run.subtests)) {
      const was = before.subtests[name];
      if (was === "PASS" && status !== "PASS") regressions.push(`${key} › ${name} (${status})`);
      if (was && was !== "PASS" && status === "PASS") progressions.push(`${key} › ${name}`);
    }
    if (before.harness === "OK" && run.harness !== "OK") {
      regressions.push(`${key} — harness ${run.harness}`);
    }
  }
  if (progressions.length > 0) {
    console.log(`\n${progressions.length} newly passing — update expectations.json:`);
    for (const line of progressions) console.log(`  ${line}`);
  }
  if (regressions.length > 0) {
    console.log(`\n${regressions.length} regression(s):`);
    for (const line of regressions) console.log(`  ${line}`);
  }
}

if (regressions.length > 0) throw new Error(`${regressions.length} WPT regression(s)`);
