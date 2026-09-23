// Runs upstream WPT files in headless Chrome and reports each subtest.
//
// The reason this exists: a failing WPT subtest is not by itself a defect here.
// Some of them test behaviour no browser has shipped — `dom/events/relatedTarget`
// has three that Chrome fails — and Chrome is this DOM's oracle everywhere else
// in `wpt/`. So before a failure becomes work, ask Chrome the same question:
//
//   tsr test:dom-wpt-chrome -- dom/events/relatedTarget.window.js
//   tsr test:dom-wpt-chrome -- --gaps
//
// A subtest Chrome fails too is upstream running ahead of the browsers, and is
// recorded as an expectation rather than fixed. One Chrome passes and this DOM
// does not is a gap, and belongs in the next commit. `--gaps` asks Chrome about
// every file with a recorded failure in `dom-expectations.json` and lists only
// the subtests Chrome passes.
//
// Pages are served from `wpt/upstream` over local HTTP, as WPT's own server
// would: an `.html` test loads as itself, and a `.any.js`/`.window.js` test
// through the wrapper page the WPT server generates for it. The report shim
// (`/resources/testharnessreport.js`) is replaced by one that hands the results
// to this script.
import puppeteer from "puppeteer-core";

const root = new URL("./upstream/", import.meta.url);
const expectationsPath = new URL("./dom-expectations.json", import.meta.url);
const flags = { chrome: "/usr/bin/google-chrome", gaps: false, json: "", jobs: 8, timeout: 15_000 };
const files = [];
for (const argument of Deno.args) {
  if (argument === "--gaps") flags.gaps = true;
  else if (argument.startsWith("--chrome=")) flags.chrome = argument.slice("--chrome=".length);
  else if (argument.startsWith("--json=")) flags.json = argument.slice("--json=".length);
  else if (argument.startsWith("--jobs=")) flags.jobs = Number(argument.slice("--jobs=".length));
  else if (argument.startsWith("--")) throw new Error(`unknown argument: ${argument}`);
  else files.push(argument);
}
const expectations = flags.gaps ? JSON.parse(await Deno.readTextFile(expectationsPath)) : null;
if (flags.gaps) {
  for (const [path, result] of Object.entries(expectations)) {
    if (result.harness !== "OK" || Object.values(result.subtests).some((status) => status !== "PASS")) files.push(path);
  }
}
if (files.length === 0) {
  console.error("usage: wpt/chrome-run.js <path under wpt/upstream>… | --gaps [--json=<path>] [--chrome=<path>]");
  Deno.exit(2);
}

const report = `add_completion_callback((tests, status) => {
  window.__wptResults = tests.map((test) => ({ name: test.name, status: test.status, message: test.message || "" }));
});`;

function wrapper(path, source) {
  const metas = [...source.matchAll(/^\/\/ META: script=(.+)$/gm)].map((match) => match[1].trim());
  const scripts = ["/resources/testharness.js", "/resources/testharnessreport.js", ...metas, `/${path}`];
  return `<!doctype html><meta charset=utf-8><body>${scripts.map((src) => `<script src="${src}"></script>`).join("")}`;
}

const types = { html: "text/html", js: "text/javascript", css: "text/css", json: "application/json", svg: "image/svg+xml" };
const server = Deno.serve({ port: 0, hostname: "127.0.0.1", onListen() {} }, async (request) => {
  const path = decodeURIComponent(new URL(request.url).pathname).replace(/^\//, "");
  if (path === "resources/testharnessreport.js") return new Response(report, { headers: { "content-type": types.js } });
  const wrapped = path.match(/^(.*\.(?:any|window))\.html$/);
  try {
    if (wrapped) {
      const source = await Deno.readTextFile(new URL(`${wrapped[1]}.js`, root));
      return new Response(wrapper(`${wrapped[1]}.js`, source), { headers: { "content-type": types.html } });
    }
    const body = await Deno.readFile(new URL(path, root));
    return new Response(body, { headers: { "content-type": types[path.split(".").pop()] ?? "application/octet-stream" } });
  } catch {
    return new Response("not found", { status: 404 });
  }
});
const origin = `http://127.0.0.1:${server.addr.port}`;
const pageOf = (path) => path.endsWith(".js") ? path.replace(/\.js$/, ".html") : path;

const names = ["PASS", "FAIL", "TIMEOUT", "NOTRUN", "PRECONDITION_FAILED"];
const browser = await puppeteer.launch({ executablePath: flags.chrome, headless: true });
const results = {};
try {
  let next = 0;
  await Promise.all(Array.from({ length: Math.max(1, flags.jobs) }, async () => {
    const tab = await browser.newPage();
    while (next < files.length) {
      const path = files[next++];
      try {
        await tab.goto(`${origin}/${pageOf(path)}`, { waitUntil: "load", timeout: flags.timeout });
        await tab.waitForFunction(() => window.__wptResults, { timeout: flags.timeout });
        const tests = await tab.evaluate(() => window.__wptResults);
        results[path] = { harness: "OK", subtests: Object.fromEntries(tests.map((test) => [test.name, names[test.status] ?? String(test.status)])), messages: Object.fromEntries(tests.map((test) => [test.name, test.message])) };
      } catch (error) {
        results[path] = { harness: "ERROR", subtests: {}, messages: {}, message: String(error.message ?? error).split("\n")[0] };
      }
    }
    await tab.close();
  }));
} finally {
  await browser.close();
  await server.shutdown();
}

if (flags.json) {
  const sorted = Object.fromEntries(Object.entries(results).sort(([a], [b]) => a.localeCompare(b)));
  await Deno.writeTextFile(flags.json, `${JSON.stringify(sorted, null, 2)}\n`);
}

if (flags.gaps) {
  // What Chrome passes and this DOM does not: the list of real work.
  let total = 0;
  for (const path of Object.keys(results).sort()) {
    const ours = expectations[path];
    const gaps = Object.entries(results[path].subtests)
      .filter(([name, status]) => status === "PASS" && (ours.harness !== "OK" || ours.subtests[name] !== "PASS"));
    if (gaps.length === 0) continue;
    total += gaps.length;
    console.log(`${gaps.length}  ${path}${ours.harness !== "OK" ? `  (here: ${ours.harness})` : ""}`);
  }
  console.log(`\n${total} subtests pass in Chrome and not here`);
} else {
  for (const path of files) {
    const result = results[path];
    if (files.length > 1) console.log(`\n# ${path}`);
    if (result.harness !== "OK") console.log(`ERROR  ${result.message}`);
    for (const [name, status] of Object.entries(result.subtests)) {
      console.log(`${status}  ${name}`);
      if (result.messages[name]) console.log(`      ${result.messages[name]}`);
    }
    const all = Object.values(result.subtests);
    console.log(`\n${all.filter((status) => status === "PASS").length} of ${all.length} pass in Chrome`);
  }
}
