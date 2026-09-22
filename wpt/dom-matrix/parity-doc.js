// Generates the DOM parity documents from the recorded baselines.
//
// It reads the two committed baselines and nothing else — no Chrome, no network
// — so the numbers in the docs are the numbers the gates recorded, and CI can
// check the docs are current without a browser.
//
//   deno run --allow-read --allow-write wpt/dom-matrix/parity-doc.js
//   deno run --allow-read wpt/dom-matrix/parity-doc.js --check

const root = new URL("../../", import.meta.url);
const surfacePath = new URL("./surface-baseline.json", import.meta.url);
const matrixPath = new URL("./baseline.json", import.meta.url);
const outputs = {
  doc: new URL("docs/ESDEV-DOM-PARITY.md", root),
  site: new URL("website/app/esdev/test/dom/parity/page.mdx", root),
};

// Why esdev answers differently from Chrome, for every feature where it does.
// Generation fails if a difference has no reason here: an unexplained one is
// the thing this document exists to prevent.
const LAYOUT = "No layout: there is no box model, so there is nothing to measure or scroll.";
const RENDERING = "No rendering: the top layer, animations and transitions have nothing to paint.";
const SPECIFIED = "Specified values only: resolving one needs layout and a font.";
const LEGACY = "Legacy or superseded by a modern API that is implemented.";
const STRICT = "Deliberately strict: malformed markup is refused rather than repaired.";
const HOST = "No host resource: this DOM reaches no network, no file and no browsing context.";
const ALIAS = "Refused by name: the modern interface name is accepted, and the HTML4 alias is answered with the constructor to use instead.";
const ARTIFACT = "Harness artifact: Chrome ran the probe on about:blank, which is an opaque, insecure origin.";

const REASONS = new Map(Object.entries({
  "innerText": LAYOUT,
  "getBoundingClientRect measures": LAYOUT,
  "offsetWidth measures": LAYOUT,
  "Element.animate (WAAPI)": RENDERING,
  "document.startViewTransition": RENDERING,
  "resolved font size": SPECIFIED,
  "XPathEvaluator (document.evaluate)": LEGACY,
  "XMLSerializer": LEGACY,
  "document.write": LEGACY,
  "TouchEvent": LEGACY,
  "createEvent refuses the HTML4 aliases": ALIAS,
  "malformed HTML": STRICT,
  "misnested tags": STRICT,
  "XMLHttpRequest": HOST,
  "File / FileReader": HOST,
  "Image": HOST,
  "canvas.getContext": HOST,
  "iframe.contentWindow": HOST,
  "alert": HOST,
  "localStorage": ARTIFACT,
  "crypto.randomUUID": ARTIFACT,
}));

const RUNTIMES = ["chrome", "esdev", "jsdom", "happy-dom"];
const LABELS = { chrome: "Chrome", esdev: "esdev", jsdom: "jsdom", "happy-dom": "happy-dom" };

const surface = JSON.parse(await Deno.readTextFile(surfacePath));
const matrix = JSON.parse(await Deno.readTextFile(matrixPath));

const differences = surface.features.filter((feature) => feature.esdev !== feature.chrome);
const unexplained = differences.filter((feature) => !REASONS.has(feature.name));
if (unexplained.length > 0) {
  console.error("Every difference from Chrome needs a reason in parity-doc.js. Missing:");
  for (const feature of unexplained) console.error(`  ${feature.group} › ${feature.name}`);
  Deno.exit(1);
}

const percent = (part, whole) => `${Math.round((part / whole) * 100)}%`;
const cell = (value) => {
  if (value === true) return "yes";
  if (value === false) return "no";
  return `\`${String(value).replaceAll("|", "\\|")}\``;
};

function headline() {
  const rows = ["esdev", "jsdom", "happy-dom"].map((runtime) => {
    const agreeing = surface.summary.agreesWithChrome[runtime];
    return `| ${LABELS[runtime]} | ${agreeing} / ${surface.summary.features} | ${percent(agreeing, surface.summary.features)} |`;
  });
  return ["| Runtime | Agrees with Chrome | |", "| --- | --: | --: |", ...rows].join("\n");
}

function areas() {
  const rows = surface.groups.map((group) =>
    `| ${group.group} | ${group.features} | ${group.esdev} | ${group.jsdom} | ${group["happy-dom"]} |`);
  return ["| Area | Features | esdev | jsdom | happy-dom |", "| --- | --: | --: | --: | --: |", ...rows].join("\n");
}

function behaviour() {
  const counts = matrix.summary;
  const rows = matrix.cases.map((entry) => {
    const same = (runtime) => (JSON.stringify(entry[runtime]) === JSON.stringify(entry.chrome) ? "yes" : "no");
    return `| ${entry.name} | ${entry.group} | ${same("esdev")} | ${same("jsdom")} | ${same("happy-dom")} |`;
  });
  return {
    counts: `${counts.match} of ${matrix.cases.length} cases match Chrome exactly, and ${counts["intentional-limit"] ?? 0} is a documented limit.`,
    table: ["| Case | Area | esdev | jsdom | happy-dom |", "| --- | --- | :--: | :--: | :--: |", ...rows].join("\n"),
  };
}

function declined() {
  const byReason = new Map();
  for (const feature of differences) {
    const reason = REASONS.get(feature.name);
    if (!byReason.has(reason)) byReason.set(reason, []);
    byReason.get(reason).push(feature);
  }
  const sections = [];
  for (const [reason, list] of byReason) {
    const rows = list.map((feature) => `| ${feature.name} | ${cell(feature.chrome)} | ${cell(feature.esdev)} |`);
    sections.push([
      `**${reason}**`,
      "",
      "| Feature | Chrome | esdev |",
      "| --- | --- | --- |",
      ...rows,
    ].join("\n"));
  }
  return sections.join("\n\n");
}

// The rows where esdev matches Chrome and an emulator does not: what this DOM
// buys a suite that would otherwise run on one of them.
function ahead() {
  const rows = surface.features
    .filter((feature) => feature.esdev === feature.chrome && (feature.jsdom !== feature.chrome || feature["happy-dom"] !== feature.chrome))
    .map((feature) => `| ${feature.name} | ${cell(feature.chrome)} | ${cell(feature.jsdom)} | ${cell(feature["happy-dom"])} |`);
  return {
    count: rows.length,
    table: ["| Feature | Chrome & esdev | jsdom | happy-dom |", "| --- | --- | --- | --- |", ...rows].join("\n"),
  };
}

function everyFeature() {
  const rows = surface.features.map((feature) =>
    `| ${feature.group} | ${feature.name} | ${cell(feature.chrome)} | ${cell(feature.esdev)} | ${cell(feature.jsdom)} | ${cell(feature["happy-dom"])} |`);
  return ["| Area | Feature | Chrome | esdev | jsdom | happy-dom |", "| --- | --- | --- | --- | --- | --- |", ...rows].join("\n");
}

const behaviours = behaviour();
const lead = ahead();

const doc = `<!-- Generated by \`tsr docs:parity\`. Edit wpt/dom-matrix/parity-doc.js, not this file. -->

# esdev DOM parity

What \`esdev test --dom\` answers, next to Chrome, jsdom and happy-dom. Chrome is
the oracle; the numbers come from the two recorded baselines in
\`wpt/dom-matrix/\`, which \`tsr test:dom-surface\` and \`tsr test:dom-matrix\`
keep current.

## Surface

${surface.summary.features} features — interfaces, members and behaviours — put to all four runtimes.

${headline()}

${areas()}

## Behaviour

${behaviours.counts} Each case runs the same code in all four runtimes and
compares the result.

${behaviours.table}

## Where esdev is closer to Chrome than an emulator

${lead.count} of the ${surface.summary.features} features.

${lead.table}

## Where esdev differs from Chrome, and why

${differences.length} of ${surface.summary.features}. Every one of them is here:
a difference with no entry fails \`tsr docs:parity\`.

${declined()}

## Every feature

${everyFeature()}
`;

const site = `---
title: DOM parity
description: What esdev's test DOM answers, next to Chrome, jsdom and happy-dom.
---

{/* Generated by \`tsr docs:parity\`. Edit wpt/dom-matrix/parity-doc.js, not this file. */}

# DOM parity

Chrome is the oracle. ${surface.summary.features} features — interfaces, members and behaviours — are
put to all four runtimes, and ${matrix.cases.length} behaviour cases run the same code in each.

${headline()}

## By area

${areas()}

## Behaviour cases

${behaviours.counts}

${behaviours.table}

## Closer to Chrome than an emulator

${lead.count} features where esdev matches Chrome and jsdom or happy-dom does not.

${lead.table}

## Differences from Chrome

${differences.length} features, each with its reason. There are no others.

${declined()}
`;

if (Deno.args.includes("--check")) {
  let stale = false;
  for (const [name, path] of Object.entries(outputs)) {
    const expected = name === "doc" ? doc : site;
    const actual = await Deno.readTextFile(path).catch(() => null);
    if (actual !== expected) {
      console.error(`${path.pathname.replace(root.pathname, "")} is out of date — run \`tsr docs:parity\`.`);
      stale = true;
    }
  }
  if (stale) Deno.exit(1);
  console.log("the parity documents match the recorded baselines");
} else {
  await Deno.mkdir(new URL("./", outputs.site), { recursive: true });
  await Deno.writeTextFile(outputs.doc, doc);
  await Deno.writeTextFile(outputs.site, site);
  console.log(`wrote docs/ESDEV-DOM-PARITY.md and website/app/esdev/test/dom/parity/page.mdx`);
}
