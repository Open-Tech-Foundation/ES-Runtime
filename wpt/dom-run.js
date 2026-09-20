// Runs the modern, layout-free JavaScript DOM WPT slice under `esdev test --dom`.
import { excluded } from "./dom-scope.js";

const root = new URL("./upstream/", import.meta.url);
const esdev = new URL("../target/debug/esdev", import.meta.url).pathname;
const roots = ["dom", "custom-elements", "shadow-dom"];
const marker = "__ESDEV_WPT_DOM__=";

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

const harness = (await Deno.readTextFile(new URL("resources/testharness.js", root)))
  // The shell environment reports completion without attempting browser UI.
  .replace("if ('document' in global_scope) {", "if (false) {")
  // The runtime's native AbortSignal predates the test DOM's Event realm.
  // WPT uses this controller only to cancel its own cleanup callbacks.
  .replace("if (typeof AbortController === \"function\") {", "if (false) {");

async function run(testPath) {
  const source = await Deno.readTextFile(new URL(testPath, root));
  const dependencies = await Promise.all(metaScripts(source).map((specifier) => Deno.readTextFile(scriptPath(specifier, testPath))));
  const generated = testPath.replace(/\.js$/, ".__esdev-dom-wpt.test.mjs");
  const collector = `add_completion_callback((tests, status) => console.log(${JSON.stringify(marker)} + JSON.stringify({ status: status.status, tests: tests.map((test) => ({ name: test.name, status: test.status, message: test.message || "" })) })));`;
  await Deno.writeTextFile(new URL(generated, root), `(0,eval)(${JSON.stringify([harness, collector, ...dependencies, source].join("\n;\n"))});\n`);
  try {
    const output = await new Deno.Command(esdev, {
      args: ["test", "--dom", `--file=${generated}`], cwd: root.pathname, stdout: "piped", stderr: "piped",
    }).output();
    const stdout = new TextDecoder().decode(output.stdout);
    const report = stdout.match(new RegExp(`^${marker}(.+)$`, "m"))?.[1];
    if (!report) return { harness: "ERROR", tests: [], message: `${stdout}${new TextDecoder().decode(output.stderr)}`.trim() };
    return { harness: "OK", ...JSON.parse(report) };
  } finally {
    await Deno.remove(new URL(generated, root)).catch(() => {});
  }
}

const selected = (await Promise.all(roots.map(files))).flat().sort();
const skipped = selected.filter((path) => excluded(path));
const runnable = selected.filter((path) => !excluded(path));
const totals = { files: selected.length, runnable: runnable.length, skipped: skipped.length, passed: 0, failed: 0, errored: 0 };
const failures = [];
for (const path of runnable) {
  const result = await run(path);
  if (result.harness !== "OK") {
    totals.errored++;
    failures.push({ path, harness: result.harness, message: result.message });
    continue;
  }
  for (const test of result.tests) {
    if (test.status === 0) totals.passed++;
    else { totals.failed++; failures.push({ path, ...test }); }
  }
}
console.log(JSON.stringify({ totals, skipped, failures }, null, 2));
