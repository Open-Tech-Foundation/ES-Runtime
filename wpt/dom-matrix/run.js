// Compare layout-free DOM cases against headless Chrome, with Node DOM emulators
// retained as compatibility context only.
import { Window } from "happy-dom";
import { JSDOM } from "jsdom";
import puppeteer from "puppeteer-core";
import { cases, runCases } from "./cases.js";
import { classify, drifted, equivalent, outcome } from "./report.js";

const root = new URL("../../", import.meta.url);
const defaultEsdev = new URL("target/debug/esdev", root).pathname;
const baselinePath = new URL("./baseline.json", import.meta.url);
const flags = {
  chrome: "/usr/bin/google-chrome",
  esdev: defaultEsdev,
  json: "",
  strict: false,
  updateBaseline: false,
};

for (const argument of Deno.args) {
  if (argument === "--strict") flags.strict = true;
  else if (argument === "--update-baseline") flags.updateBaseline = true;
  else if (argument.startsWith("--chrome=")) {
    flags.chrome = argument.slice("--chrome=".length);
  } else if (argument.startsWith("--esdev=")) {
    flags.esdev = argument.slice("--esdev=".length);
  } else if (argument.startsWith("--json=")) {
    flags.json = argument.slice("--json=".length);
  } else throw new Error(`unknown argument: ${argument}`);
}

async function runEsdev() {
  const directory = await Deno.makeTempDir({ prefix: "esdev-dom-matrix-" });
  const testFile = `${directory}/matrix.test.mjs`;
  const casesFile = new URL("./cases.js", import.meta.url);
  await Deno.writeTextFile(
    `${directory}/cases.js`,
    await Deno.readTextFile(casesFile),
  );
  await Deno.writeTextFile(
    testFile,
    'import { test } from "runtime:test";\nimport { runCases } from "./cases.js";\ntest("DOM matrix", () => console.log("DOM_MATRIX=" + JSON.stringify(runCases(globalThis))));\n',
  );
  try {
    const output = await new Deno.Command(flags.esdev, {
      args: ["test", "--dom", "--file=matrix.test.mjs"],
      cwd: directory,
      stderr: "piped",
      stdout: "piped",
    }).output();
    const stdout = new TextDecoder().decode(output.stdout);
    const encoded = stdout.match(/^DOM_MATRIX=(.+)$/m)?.[1];
    if (!output.success || !encoded) {
      const stderr = new TextDecoder().decode(output.stderr);
      throw new Error(`esdev matrix failed:\n${stdout}${stderr}`);
    }
    return JSON.parse(encoded);
  } finally {
    await Deno.remove(directory, { recursive: true });
  }
}

function runJsdom() {
  const dom = new JSDOM(
    '<!doctype html><html><head><base href="http://localhost/"></head><body></body></html>',
    {
      url: "http://localhost/",
    },
  );
  try {
    return runCases(dom.window);
  } finally {
    dom.window.close();
  }
}

function runHappyDom() {
  const window = new Window({ url: "http://localhost/" });
  window.document.head.innerHTML = '<base href="http://localhost/">';
  try {
    return runCases(window);
  } finally {
    window.close();
  }
}

async function runChrome() {
  const browser = await puppeteer.launch({
    executablePath: flags.chrome,
    headless: true,
  });
  try {
    const page = await browser.newPage();
    await page.setContent(
      '<!doctype html><html><head><base href="http://localhost/"></head><body></body></html>',
    );
    const source = await Deno.readTextFile(
      new URL("./cases.js", import.meta.url),
    );
    return await page.evaluate(async (moduleSource) => {
      const url = URL.createObjectURL(
        new Blob([moduleSource], { type: "text/javascript" }),
      );
      try {
        const { runCases } = await import(url);
        return runCases(globalThis);
      } finally {
        URL.revokeObjectURL(url);
      }
    }, source);
  } finally {
    await browser.close();
  }
}

const [chrome, esdev, jsdom, happyDom] = await Promise.all([
  runChrome(),
  runEsdev(),
  runJsdom(),
  runHappyDom(),
]);
// Keyed by name rather than by position: each runtime runs the same cases in
// the same order today, but an index says nothing about which case it is, and a
// mis-aligned column would read as a gap in whichever case it landed on.
function byName(runtime, results) {
  const found = new Map(results.map((result) => [result.name, result]));
  for (const { name } of cases) {
    if (!found.has(name)) throw new Error(`${runtime} reported no result for ${name}`);
  }
  return found;
}

const results = {
  chrome: byName("chrome", chrome),
  esdev: byName("esdev", esdev),
  "happy-dom": byName("happy-dom", happyDom),
  jsdom: byName("jsdom", jsdom),
};
const baseline = new Map(
  (await Deno.readTextFile(baselinePath).then(JSON.parse).catch(() => ({ cases: [] })))
    .cases.map((recorded) => [recorded.name, recorded]),
);
const report = cases.map((test) => {
  const values = {
    chrome: outcome(results.chrome.get(test.name)),
    esdev: outcome(results.esdev.get(test.name)),
    "happy-dom": outcome(results["happy-dom"].get(test.name)),
    jsdom: outcome(results.jsdom.get(test.name)),
  };
  const emulatorDisagreement = !equivalent(values.jsdom, values["happy-dom"]) ||
    !equivalent(values.chrome, values.jsdom) ||
    !equivalent(values.chrome, values["happy-dom"]);
  return {
    ...test,
    ...values,
    emulatorDisagreement,
    status: classify(test, values),
    drift: drifted(baseline.get(test.name), values),
    run: undefined,
  };
});

const counts = Object.groupBy(report, ({ status }) => status);
const summary = Object.fromEntries(
  Object.entries(counts).map(([status, entries]) => [status, entries.length]),
);
const drift = report.filter(({ drift: changed }) => changed).map(({ name }) => name);
const output = { cases: report, summary: { ...summary, drift: drift.length } };
if (flags.json) {
  await Deno.writeTextFile(flags.json, `${JSON.stringify(output, null, 2)}\n`);
}
if (flags.updateBaseline) {
  await Deno.writeTextFile(baselinePath, `${JSON.stringify(output, null, 2)}\n`);
}
console.log(JSON.stringify(output, null, 2));
// A recorded run that nothing compares against rots in place. `drift` is any
// case whose four columns no longer read the way baseline.json says they did —
// including one of the emulators moving, which is the whole reason they are
// here. Re-record with --update-baseline in the commit that explains it.
if (flags.strict && !flags.updateBaseline && (summary.gap || drift.length)) {
  console.error(`gaps: ${summary.gap ?? 0}; drifted from the baseline: ${drift.join(", ") || "none"}`);
  Deno.exit(1);
}
