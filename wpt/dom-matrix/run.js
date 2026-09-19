// Compare layout-free DOM cases under esdev, jsdom, and happy-dom.
import { Window } from "happy-dom";
import { JSDOM } from "jsdom";
import { cases, runCases } from "./cases.js";

const root = new URL("../../", import.meta.url);
const defaultEsdev = new URL("target/debug/esdev", root).pathname;
const flags = { esdev: defaultEsdev, json: "", strict: false };

for (const argument of Deno.args) {
  if (argument === "--strict") flags.strict = true;
  else if (argument.startsWith("--esdev=")) {
    flags.esdev = argument.slice("--esdev=".length);
  } else if (argument.startsWith("--json=")) {
    flags.json = argument.slice("--json=".length);
  } else throw new Error(`unknown argument: ${argument}`);
}

function equivalent(left, right) {
  return JSON.stringify(left) === JSON.stringify(right);
}

function outcome(result) {
  return Object.hasOwn(result, "error")
    ? { error: result.error }
    : { result: result.result };
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
  const dom = new JSDOM("<!doctype html><html><body></body></html>", {
    url: "https://matrix.invalid/",
  });
  try {
    return runCases(dom.window);
  } finally {
    dom.window.close();
  }
}

function runHappyDom() {
  const window = new Window({ url: "https://matrix.invalid/" });
  try {
    return runCases(window);
  } finally {
    window.close();
  }
}

const [esdev, jsdom, happyDom] = await Promise.all([
  runEsdev(),
  runJsdom(),
  runHappyDom(),
]);
const report = cases.map((test, index) => {
  const values = {
    esdev: outcome(esdev[index]),
    "happy-dom": outcome(happyDom[index]),
    jsdom: outcome(jsdom[index]),
  };
  const referenceMatch = equivalent(values.jsdom, values["happy-dom"]);
  let status = "gap";
  if (test.limit && equivalent(values.esdev, test.expectedEsdev)) {
    status = "intentional-limit";
  } else if (referenceMatch && equivalent(values.esdev, values.jsdom)) {
    status = "match";
  } else if (!referenceMatch) status = "reference-disagreement";
  return { ...test, ...values, status, run: undefined };
});

const counts = Object.groupBy(report, ({ status }) => status);
const summary = Object.fromEntries(
  Object.entries(counts).map(([status, entries]) => [status, entries.length]),
);
const output = { cases: report, summary };
if (flags.json) {
  await Deno.writeTextFile(flags.json, `${JSON.stringify(output, null, 2)}\n`);
}
console.log(JSON.stringify(output, null, 2));
if (flags.strict && (summary.gap || summary["reference-disagreement"])) {
  Deno.exit(1);
}
