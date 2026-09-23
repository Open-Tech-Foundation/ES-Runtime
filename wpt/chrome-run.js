// Runs one upstream WPT file in headless Chrome and reports each subtest.
//
// The reason this exists: a failing WPT subtest is not by itself a defect here.
// Some of them test behaviour no browser has shipped — `dom/events/relatedTarget`
// has three that Chrome fails — and Chrome is this DOM's oracle everywhere else
// in `wpt/`. So before a failure becomes work, ask Chrome the same question:
//
//   tsr test:dom-wpt-chrome -- dom/events/relatedTarget.window.js
//
// A subtest Chrome fails too is upstream running ahead of the browsers, and is
// recorded as an expectation rather than fixed. One Chrome passes and this DOM
// does not is a gap, and belongs in the next commit.
import puppeteer from "puppeteer-core";

const file = Deno.args.find((argument) => !argument.startsWith("--"));
if (!file) {
  console.error("usage: wpt/chrome-run.js <path under wpt/upstream> [--chrome=/path/to/chrome]");
  Deno.exit(2);
}
const chrome = Deno.args.find((argument) => argument.startsWith("--chrome="))?.slice("--chrome=".length)
  ?? "/usr/bin/google-chrome";

const root = new URL("./upstream/", import.meta.url);
const harness = await Deno.readTextFile(new URL("resources/testharness.js", root));
const source = await Deno.readTextFile(new URL(file, root));

const browser = await puppeteer.launch({ executablePath: chrome, headless: true });
try {
  const tab = await browser.newPage();
  // A document in standards mode with a body, which is what a `.window.js` test
  // expects to find.
  await tab.setContent("<!doctype html><html><body></body></html>");
  await tab.evaluate(harness);
  const results = await tab.evaluate(
    (text) =>
      new Promise((resolve) => {
        // `add_completion_callback` is the harness's own reporting hook, so the
        // statuses are the ones the runner in this directory compares against.
        add_completion_callback((tests) =>
          resolve(tests.map((test) => ({ name: test.name, status: test.status, message: test.message }))));
        const script = document.createElement("script");
        script.textContent = text;
        document.head.appendChild(script);
        done();
      }),
    source,
  );
  const names = ["PASS", "FAIL", "TIMEOUT", "NOTRUN", "PRECONDITION_FAILED"];
  for (const result of results) {
    console.log(`${names[result.status] ?? result.status}  ${result.name}`);
    if (result.message) console.log(`      ${result.message}`);
  }
  const failed = results.filter((result) => result.status !== 0).length;
  console.log(`\n${results.length - failed} of ${results.length} pass in Chrome`);
} finally {
  await browser.close();
}
