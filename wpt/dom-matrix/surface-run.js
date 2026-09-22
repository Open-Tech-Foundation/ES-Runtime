// Puts the surface probe to Chrome, esdev, jsdom and happy-dom, and compares
// the four answers against the recorded baseline.
//
// Unlike the behaviour matrix, a difference from Chrome here is usually a stated
// non-goal rather than a defect — a layout API cannot be answered by a DOM with
// no layout. So this run reports rather than judges, and what it gates on is
// *drift*: an answer that no longer matches the record.

import { Window } from "happy-dom";
import { JSDOM } from "jsdom";
import puppeteer from "puppeteer-core";
import { features, probe } from "./surface.js";

const root = new URL("../../", import.meta.url);
const baselinePath = new URL("./surface-baseline.json", import.meta.url);
const page = '<!doctype html><html><head><base href="http://localhost/"></head><body></body></html>';
const flags = {
  chrome: "/usr/bin/google-chrome",
  esdev: new URL("target/debug/esdev", root).pathname,
  json: "",
  strict: false,
  updateBaseline: false,
};

for (const argument of Deno.args) {
  if (argument === "--strict") flags.strict = true;
  else if (argument === "--update-baseline") flags.updateBaseline = true;
  else if (argument.startsWith("--chrome=")) flags.chrome = argument.slice("--chrome=".length);
  else if (argument.startsWith("--esdev=")) flags.esdev = argument.slice("--esdev=".length);
  else if (argument.startsWith("--json=")) flags.json = argument.slice("--json=".length);
  else throw new Error(`unknown argument: ${argument}`);
}

async function runEsdev() {
  const directory = await Deno.makeTempDir({ prefix: "esdev-dom-surface-" });
  await Deno.writeTextFile(`${directory}/surface.js`, await Deno.readTextFile(new URL("./surface.js", import.meta.url)));
  await Deno.writeTextFile(
    `${directory}/surface.test.mjs`,
    'import { test } from "runtime:test";\nimport { probe } from "./surface.js";\ntest("DOM surface", () => console.log("DOM_SURFACE=" + JSON.stringify(probe(globalThis))));\n',
  );
  try {
    const output = await new Deno.Command(flags.esdev, {
      args: ["test", "--dom", "--file=surface.test.mjs"],
      cwd: directory,
      stderr: "piped",
      stdout: "piped",
    }).output();
    const stdout = new TextDecoder().decode(output.stdout);
    const encoded = stdout.match(/^DOM_SURFACE=(.+)$/m)?.[1];
    if (!output.success || !encoded) {
      throw new Error(`esdev surface failed (exit ${output.code}):\n${stdout}${new TextDecoder().decode(output.stderr)}`);
    }
    return JSON.parse(encoded);
  } finally {
    await Deno.remove(directory, { recursive: true });
  }
}

function runJsdom() {
  const dom = new JSDOM(page, { url: "http://localhost/", pretendToBeVisual: true });
  try {
    return probe(dom.window);
  } finally {
    dom.window.close();
  }
}

function runHappyDom() {
  const window = new Window({ url: "http://localhost/" });
  try {
    return probe(window);
  } finally {
    window.close();
  }
}

async function runChrome() {
  const browser = await puppeteer.launch({ executablePath: flags.chrome, headless: true });
  try {
    const tab = await browser.newPage();
    await tab.setContent(page);
    const source = await Deno.readTextFile(new URL("./surface.js", import.meta.url));
    return await tab.evaluate(async (moduleSource) => {
      const url = URL.createObjectURL(new Blob([moduleSource], { type: "text/javascript" }));
      try {
        const { probe } = await import(url);
        return probe(globalThis);
      } finally {
        URL.revokeObjectURL(url);
      }
    }, source);
  } finally {
    await browser.close();
  }
}

// One runtime failing to start should not cost the report the other three.
async function attempt(name, runner) {
  try {
    return await runner();
  } catch (error) {
    console.error(`${name}: ${error.message}`);
    return features.map(([group, feature]) => ({ group, name: feature, answer: "runner-failed" }));
  }
}

const [chrome, esdev, jsdom, happyDom] = await Promise.all([
  attempt("chrome", runChrome),
  attempt("esdev", runEsdev),
  attempt("jsdom", runJsdom),
  attempt("happy-dom", runHappyDom),
]);

// Keyed by name, not by position: an index says nothing about which feature it
// is, and a column that slipped would read as a difference in the wrong row.
function byName(runtime, results) {
  const found = new Map(results.map((entry) => [entry.name, entry.answer]));
  for (const [, name] of features) {
    if (!found.has(name)) throw new Error(`${runtime} answered nothing for ${name}`);
  }
  return found;
}

const columns = {
  chrome: byName("chrome", chrome),
  esdev: byName("esdev", esdev),
  jsdom: byName("jsdom", jsdom),
  "happy-dom": byName("happy-dom", happyDom),
};
const runtimes = ["chrome", "esdev", "jsdom", "happy-dom"];
const baseline = new Map(
  (await Deno.readTextFile(baselinePath).then(JSON.parse).catch(() => ({ features: [] })))
    .features.map((recorded) => [recorded.name, recorded]),
);

const report = features.map(([group, name]) => {
  const row = { group, name };
  for (const runtime of runtimes) row[runtime] = columns[runtime].get(name);
  const recorded = baseline.get(name);
  row.drift = !recorded || runtimes.some((runtime) => recorded[runtime] !== row[runtime]);
  return row;
});

const agreeing = (runtime) => report.filter((row) => row[runtime] === row.chrome).length;
const summary = {
  features: report.length,
  agreesWithChrome: Object.fromEntries(runtimes.slice(1).map((runtime) => [runtime, agreeing(runtime)])),
  drift: report.filter((row) => row.drift).length,
};
const groups = [...new Set(report.map((row) => row.group))].map((group) => {
  const rows = report.filter((row) => row.group === group);
  return {
    group,
    features: rows.length,
    ...Object.fromEntries(runtimes.slice(1).map((runtime) => [runtime, rows.filter((row) => row[runtime] === row.chrome).length])),
  };
});

const output = { summary, groups, features: report };
if (flags.json) await Deno.writeTextFile(flags.json, `${JSON.stringify(output, null, 2)}\n`);
if (flags.updateBaseline) {
  const recorded = {
    summary: { ...summary, drift: 0 },
    groups,
    features: report.map((row) => ({ ...row, drift: false })),
  };
  await Deno.writeTextFile(baselinePath, `${JSON.stringify(recorded, null, 2)}\n`);
}
console.log(JSON.stringify(output, null, 2));

if (flags.strict && !flags.updateBaseline && summary.drift > 0) {
  console.error(`\n${summary.drift} answer(s) drifted from wpt/dom-matrix/surface-baseline.json:`);
  for (const row of report.filter((entry) => entry.drift)) console.error(`  ${row.group} › ${row.name}`);
  console.error("\nRe-record with --update-baseline in the commit that explains the change.");
  Deno.exit(1);
}
