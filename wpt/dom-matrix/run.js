// Compare layout-free DOM cases against headless Chrome, with Node DOM emulators
// retained as compatibility context only.
import { Window } from "happy-dom";
import { JSDOM } from "jsdom";
import puppeteer from "puppeteer-core";
import { cases, runCases } from "./cases.js";
import { classify, equivalent, outcome } from "./report.js";

const root = new URL("../../", import.meta.url);
const defaultEsdev = new URL("target/debug/esdev", root).pathname;
const flags = {
  chrome: "/usr/bin/google-chrome",
  esdev: defaultEsdev,
  json: "",
  strict: false,
};

for (const argument of Deno.args) {
  if (argument === "--strict") flags.strict = true;
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
const report = cases.map((test, index) => {
  const values = {
    chrome: outcome(chrome[index]),
    esdev: outcome(esdev[index]),
    "happy-dom": outcome(happyDom[index]),
    jsdom: outcome(jsdom[index]),
  };
  const emulatorDisagreement = !equivalent(values.jsdom, values["happy-dom"]) ||
    !equivalent(values.chrome, values.jsdom) ||
    !equivalent(values.chrome, values["happy-dom"]);
  return {
    ...test,
    ...values,
    emulatorDisagreement,
    status: classify(test, values),
    run: undefined,
  };
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
if (flags.strict && summary.gap) {
  Deno.exit(1);
}
