// Runs the modern, layout-free JavaScript DOM WPT slice under `esdev test --dom`.
import { excluded } from "./dom-scope.js";

const root = new URL("./upstream/", import.meta.url);
const defaultEsdev = new URL("../target/debug/esdev", import.meta.url).pathname;
const roots = ["dom", "custom-elements", "shadow-dom"];
const marker = "__ESDEV_WPT_DOM__=";
const expectationsPath = new URL("./dom-expectations.json", import.meta.url);
// testharness.js subtest statuses, by their numeric value.
const statusNames = ["PASS", "FAIL", "TIMEOUT", "NOTRUN", "PRECONDITION_FAILED"];

const flags = { esdev: defaultEsdev, filter: "", json: "", timeout: 10_000, update: false, verbose: false, keep: false, jobs: Math.min(navigator.hardwareConcurrency ?? 4, 8) };
for (const argument of Deno.args) {
  if (argument === "--verbose") flags.verbose = true;
  else if (argument === "--keep") flags.keep = true;
  else if (argument.startsWith("--jobs=")) flags.jobs = Number(argument.slice("--jobs=".length));
  else if (argument === "--update-expectations") flags.update = true;
  else if (argument.startsWith("--esdev=")) flags.esdev = argument.slice("--esdev=".length);
  else if (argument.startsWith("--filter=")) flags.filter = argument.slice("--filter=".length);
  else if (argument.startsWith("--json=")) flags.json = argument.slice("--json=".length);
  else if (argument.startsWith("--timeout=")) flags.timeout = Number(argument.slice("--timeout=".length));
  else throw new Error(`unknown argument: ${argument}`);
}
if (!Number.isFinite(flags.timeout) || flags.timeout <= 0) throw new Error("--timeout wants milliseconds");

// A test is a `.any.js`/`.window.js` file, or an `.html` page that loads
// testharness.js — which leaves out reftests, crash tests and the helper pages
// under `resources/` and `support/`, none of which report subtests.
async function files(directory) {
  const found = [];
  for await (const entry of Deno.readDir(new URL(`${directory}/`, root))) {
    const path = `${directory}/${entry.name}`;
    if (entry.isDirectory) {
      if (entry.name !== "resources" && entry.name !== "support") found.push(...await files(path));
    } else if (entry.isFile && (path.endsWith(".any.js") || path.endsWith(".window.js"))) found.push(path);
    else if (entry.isFile && path.endsWith(".html")) {
      const source = await Deno.readTextFile(new URL(path, root));
      if (/<script[^>]*src=["']?\/resources\/testharness\.js/.test(source)) found.push(path);
    }
  }
  return found;
}

// The harness is loaded once, ahead of the page; a page's own `<script src>` for
// it, or for the report shim, is already satisfied.
const PROVIDED = new Set(["/resources/testharness.js", "/resources/testharnessreport.js"]);

// What runs an `.html` test: its markup becomes the document — parsed as a
// document, then adopted into this one, with its script elements left in the
// tree as a browser leaves them — and each classic script then runs in tree
// order at global scope. A script that throws is reported and the next one
// still runs, as in a page. A page the strict parser refuses is not a DOM
// failure to count but a stated limit (D93), so it is reported as such.
function pagePrelude(markup, externals) {
  return `
const markup = ${JSON.stringify(markup)};
const externals = new Map(${JSON.stringify([...externals])});
let parsed;
try {
  parsed = new DOMParser().parseFromString(markup, "text/html");
} catch (error) {
  console.log(${JSON.stringify(markupMarker)} + JSON.stringify(String(error?.message ?? error)));
}
if (parsed) {
  for (const attribute of Array.from(parsed.documentElement.attributes)) document.documentElement.setAttribute(attribute.name, attribute.value);
  for (const attribute of Array.from(parsed.body.attributes)) document.body.setAttribute(attribute.name, attribute.value);
  // Snapshot before adopting: adoption takes each node out of the live list.
  document.head.replaceChildren(...Array.from(parsed.head.childNodes).map((node) => document.adoptNode(node)));
  document.body.replaceChildren(...Array.from(parsed.body.childNodes).map((node) => document.adoptNode(node)));
  globalThis.__esdevPageScripts = Array.from(document.querySelectorAll("script")).flatMap((script) => {
    const type = (script.getAttribute("type") ?? "").trim().toLowerCase();
    if (type !== "" && type !== "text/javascript" && type !== "application/javascript") return [];
    const src = script.getAttribute("src");
    if (src === null) return [script.textContent];
    return externals.has(src) ? [externals.get(src)] : [];
  });
}
`;
}

const markupMarker = "__ESDEV_WPT_MARKUP__=";

function metaScripts(source) {
  return [...source.matchAll(/^\/\/ META: script=(.+)$/gm)].map((match) => match[1].trim());
}

function scriptPath(specifier, testPath) {
  if (specifier.startsWith("/")) return new URL(specifier.slice(1), root);
  return new URL(specifier, new URL(testPath, root));
}

// A patch that stops matching is worse than no patch: the run would go on
// against an unmodified harness and report the fallout as DOM failures. Every
// substitution names itself and has to land.
function patch(source, edits) {
  return edits.reduce((text, [what, from, to]) => {
    if (!text.includes(from)) throw new Error(`testharness.js no longer ${what}; re-check the patch against REV in fetch.sh`);
    return text.replace(from, to);
  }, source);
}

const harness = patch(await Deno.readTextFile(new URL("resources/testharness.js", root)), [
  // The shell environment reports completion without attempting browser UI.
  ["selects its environment by `document`", "if ('document' in global_scope) {", "if (false) {"],
]);

async function htmlTest(testPath, collector) {
  const markup = await Deno.readTextFile(new URL(testPath, root));
  const externals = new Map();
  for (const [, src] of markup.matchAll(/<script\b[^>]*\bsrc=["']?([^"' >]+)/g)) {
    if (PROVIDED.has(src) || externals.has(src)) continue;
    externals.set(src, await Deno.readTextFile(scriptPath(src, testPath)));
  }
  return `${pagePrelude(markup, externals)}
if (globalThis.__esdevPageScripts) {
  (0,eval)(${JSON.stringify([harness, collector].join("\n;\n"))});
  for (const script of globalThis.__esdevPageScripts) {
    try { (0,eval)(script); } catch (error) { reportError(error); }
  }
  setTimeout(() => window.dispatchEvent(new Event("load")), 0);
}
`;
}

async function run(testPath) {
  const collector = `add_completion_callback((tests, status) => console.log(${JSON.stringify(marker)} + JSON.stringify({ status: status.status, tests: tests.map((test) => ({ name: test.name, status: test.status, message: test.message || "" })) })));`;
  const generated = testPath.replace(/\.(js|html)$/, ".__esdev-dom-wpt.test.mjs");
  let body;
  try {
    if (testPath.endsWith(".html")) body = await htmlTest(testPath, collector);
    else {
      const source = await Deno.readTextFile(new URL(testPath, root));
      const dependencies = await Promise.all(metaScripts(source).map((specifier) => Deno.readTextFile(scriptPath(specifier, testPath))));
      body = `(0,eval)(${JSON.stringify([harness, collector, ...dependencies, source].join("\n;\n"))});\n`;
    }
  } catch (error) {
    return { harness: "ERROR", tests: [], message: `could not assemble the test: ${error.message}` };
  }
  await Deno.writeTextFile(new URL(generated, root), body);
  const child = new Deno.Command(flags.esdev, {
    args: ["test", "--dom", `--file=${generated}`], cwd: root.pathname, stdout: "piped", stderr: "piped",
  }).spawn();
  // `--file` is the per-file child of `esdev test`, so the timeout the parent
  // would have applied is ours to apply. A harness that never completes is a
  // defect worth reporting, not a run that never ends.
  const deadline = setTimeout(() => child.kill("SIGKILL"), flags.timeout);
  try {
    const output = await child.output();
    clearTimeout(deadline);
    const stdout = new TextDecoder().decode(output.stdout);
    const stderr = new TextDecoder().decode(output.stderr);
    const refused = stdout.match(new RegExp(`^${markupMarker}(.+)$`, "m"))?.[1];
    if (refused) return { harness: "MARKUP", tests: [], message: JSON.parse(refused) };
    const report = stdout.match(new RegExp(`^${marker}(.+)$`, "m"))?.[1];
    // Exit code included deliberately: a file whose harness never completes
    // prints nothing at all and exits 0, and "" is not a report of anything.
    if (!report) {
      const expired = output.signal !== null;
      // Exit 0 and no report: every task ran and the harness was still
      // waiting — on an event, a load, a callback that never came.
      if (!expired && output.code === 0 && stdout.trim() === "" && stderr.trim() === "") {
        return { harness: "INCOMPLETE", tests: [], message: "the harness never completed: it was waiting on something that never happened" };
      }
      return {
        harness: expired ? "TIMEOUT" : "ERROR",
        tests: [],
        message: [expired ? `no result in ${flags.timeout} ms` : `exit ${output.code}`, stdout, stderr].join("\n").trim(),
      };
    }
    return { harness: "OK", ...JSON.parse(report) };
  } finally {
    clearTimeout(deadline);
    // `--keep` leaves the generated file behind, to run by hand with `esdev test --dom --file=…`.
    if (!flags.keep) await Deno.remove(new URL(generated, root)).catch(() => {});
  }
}

// The page and the scripts it loads, which is what scope decisions read.
async function sourceOf(path) {
  const text = await Deno.readTextFile(new URL(path, root));
  const specifiers = path.endsWith(".html")
    ? [...text.matchAll(/<script\b[^>]*\bsrc=["']?([^"' >]+)/g)].map((match) => match[1]).filter((src) => !PROVIDED.has(src))
    : metaScripts(text);
  const helpers = await Promise.all(specifiers.map((specifier) => Deno.readTextFile(scriptPath(specifier, path)).catch(() => "")));
  return [text, ...helpers].join("\n");
}

const selected = (await Promise.all(roots.map(files))).flat().sort()
  .filter((path) => path.includes(flags.filter));
const reasons = new Map(await Promise.all(selected.map(async (path) => [path, excluded(path, await sourceOf(path))])));
const skipped = selected.flatMap((path) => {
  const reason = reasons.get(path);
  return reason ? [{ path, reason }] : [];
});
const runnable = selected.filter((path) => !reasons.get(path));
const totals = { files: selected.length, runnable: runnable.length, skipped: skipped.length, passed: 0, failed: 0, errored: 0, timeout: 0 };
const failures = [];
const results = {};
// Files run in parallel — each is its own process — and are accounted in path
// order afterwards, so the report does not depend on which finished first.
const outcomes = new Array(runnable.length);
let next = 0;
await Promise.all(Array.from({ length: Math.max(1, flags.jobs) }, async () => {
  while (next < runnable.length) {
    const index = next++;
    outcomes[index] = await run(runnable[index]);
    if (flags.verbose) console.error(`${outcomes[index].harness.padEnd(7)} ${runnable[index]}`);
  }
}));
for (const [index, path] of runnable.entries()) {
  const result = outcomes[index];
  if (result.harness === "MARKUP") {
    totals.skipped++;
    totals.runnable--;
    skipped.push({ path, reason: `markup the strict parser refuses (D93): ${result.message}` });
    continue;
  }
  const subtests = {};
  results[path] = { harness: result.harness, subtests };
  if (result.harness !== "OK") {
    if (result.harness === "TIMEOUT" || result.harness === "INCOMPLETE") totals.timeout++;
    else totals.errored++;
    failures.push({ path, harness: result.harness, message: result.message });
    continue;
  }
  for (const test of result.tests) {
    subtests[test.name] = statusNames[test.status] ?? `STATUS_${test.status}`;
    if (test.status === 0) totals.passed++;
    else { totals.failed++; failures.push({ path, ...test }); }
  }
}

// A recorded expectation is a floor, the same contract wpt/run.js works to: a
// subtest that used to pass and now does not fails the run, and one that starts
// passing is reported, because the record is stale and the fix should land with
// it updated.
const regressions = [];
const progressions = [];
if (flags.update) {
  await Deno.writeTextFile(expectationsPath, `${JSON.stringify(results, null, 2)}\n`);
  console.error(`recorded ${Object.keys(results).length} files to wpt/dom-expectations.json`);
} else {
  const expected = await Deno.readTextFile(expectationsPath).then(JSON.parse).catch(() => null);
  if (!expected) console.error("no wpt/dom-expectations.json yet — run with --update-expectations to record one");
  else {
    for (const [path, run] of Object.entries(results)) {
      const before = expected[path];
      if (!before) continue;
      for (const [name, status] of Object.entries(run.subtests)) {
        const was = before.subtests[name];
        if (was === "PASS" && status !== "PASS") regressions.push(`${path} › ${name} (${status})`);
        if (was && was !== "PASS" && status === "PASS") progressions.push(`${path} › ${name}`);
      }
      if (before.harness === "OK" && run.harness !== "OK") regressions.push(`${path} — harness ${run.harness}`);
    }
  }
}

const report = { totals, skipped, failures };
if (flags.json) await Deno.writeTextFile(flags.json, `${JSON.stringify(report, null, 2)}\n`);
console.log(JSON.stringify(report, null, 2));
if (progressions.length > 0) {
  console.error(`\n${progressions.length} newly passing — update wpt/dom-expectations.json:`);
  for (const line of progressions) console.error(`  ${line}`);
}
if (regressions.length > 0) {
  console.error(`\n${regressions.length} regression(s):`);
  for (const line of regressions) console.error(`  ${line}`);
  Deno.exit(1);
}
