// Runs the modern, layout-free JavaScript DOM WPT slice under `esdev test --dom`.
import { excluded } from "./dom-scope.js";

const root = new URL("./upstream/", import.meta.url);
const defaultEsdev = new URL("../target/debug/esdev", import.meta.url).pathname;
const roots = ["dom", "custom-elements", "shadow-dom"];
const marker = "__ESDEV_WPT_DOM__=";

const flags = { esdev: defaultEsdev, filter: "", json: "", timeout: 10_000, verbose: false };
for (const argument of Deno.args) {
  if (argument === "--verbose") flags.verbose = true;
  else if (argument.startsWith("--esdev=")) flags.esdev = argument.slice("--esdev=".length);
  else if (argument.startsWith("--filter=")) flags.filter = argument.slice("--filter=".length);
  else if (argument.startsWith("--json=")) flags.json = argument.slice("--json=".length);
  else if (argument.startsWith("--timeout=")) flags.timeout = Number(argument.slice("--timeout=".length));
  else throw new Error(`unknown argument: ${argument}`);
}
if (!Number.isFinite(flags.timeout) || flags.timeout <= 0) throw new Error("--timeout wants milliseconds");

async function files(directory) {
  const found = [];
  for await (const entry of Deno.readDir(new URL(`${directory}/`, root))) {
    const path = `${directory}/${entry.name}`;
    if (entry.isDirectory) found.push(...await files(path));
    else if (entry.isFile && (path.endsWith(".any.js") || path.endsWith(".window.js"))) found.push(path);
  }
  return found;
}

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

async function run(testPath) {
  const source = await Deno.readTextFile(new URL(testPath, root));
  const dependencies = await Promise.all(metaScripts(source).map((specifier) => Deno.readTextFile(scriptPath(specifier, testPath))));
  const generated = testPath.replace(/\.js$/, ".__esdev-dom-wpt.test.mjs");
  const collector = `add_completion_callback((tests, status) => console.log(${JSON.stringify(marker)} + JSON.stringify({ status: status.status, tests: tests.map((test) => ({ name: test.name, status: test.status, message: test.message || "" })) })));`;
  await Deno.writeTextFile(new URL(generated, root), `(0,eval)(${JSON.stringify([harness, collector, ...dependencies, source].join("\n;\n"))});\n`);
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
    const report = stdout.match(new RegExp(`^${marker}(.+)$`, "m"))?.[1];
    // Exit code included deliberately: a file whose harness never completes
    // prints nothing at all and exits 0, and "" is not a report of anything.
    if (!report) {
      const expired = output.signal !== null;
      return {
        harness: expired ? "TIMEOUT" : "ERROR",
        tests: [],
        message: [expired ? `no result in ${flags.timeout} ms` : `exit ${output.code}`, stdout, stderr].join("\n").trim(),
      };
    }
    return { harness: "OK", ...JSON.parse(report) };
  } finally {
    clearTimeout(deadline);
    await Deno.remove(new URL(generated, root)).catch(() => {});
  }
}

const selected = (await Promise.all(roots.map(files))).flat().sort()
  .filter((path) => path.includes(flags.filter));
const skipped = selected.flatMap((path) => {
  const reason = excluded(path);
  return reason ? [{ path, reason }] : [];
});
const runnable = selected.filter((path) => !excluded(path));
const totals = { files: selected.length, runnable: runnable.length, skipped: skipped.length, passed: 0, failed: 0, errored: 0, timeout: 0 };
const failures = [];
for (const path of runnable) {
  const result = await run(path);
  if (flags.verbose) console.error(`${result.harness.padEnd(7)} ${path}`);
  if (result.harness !== "OK") {
    if (result.harness === "TIMEOUT") totals.timeout++;
    else totals.errored++;
    failures.push({ path, harness: result.harness, message: result.message });
    continue;
  }
  for (const test of result.tests) {
    if (test.status === 0) totals.passed++;
    else { totals.failed++; failures.push({ path, ...test }); }
  }
}
const report = { totals, skipped, failures };
if (flags.json) await Deno.writeTextFile(flags.json, `${JSON.stringify(report, null, 2)}\n`);
console.log(JSON.stringify(report, null, 2));
