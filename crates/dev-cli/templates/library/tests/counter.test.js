import { assert, test } from "runtime:test";
import { exists, file } from "runtime:fs";

// The component itself needs a DOM and the OTF compiler, so it cannot run
// under `esdev test` — what runs here is the consumer contract instead: the
// manifest entry resolves to a real file, and the component source exports a
// default Counter with an `initial` prop. A click-through test needs a
// browser runner, which is outside this starter.
// Reads are anchored at this file's directory, like the imports in the
// upstream test this replaces (`../src/Counter.jsx`) — so the manifest and
// sources are one level up.
async function manifest() {
  return JSON.parse(await file("../package.json").text());
}

test("the manifest entry resolves to a file that exists", async () => {
  const entry = (await manifest()).exports?.["."];
  assert(typeof entry === "string", 'exports["."] names the entry');
  assert(entry.startsWith("./"), `the entry is a relative path: ${entry}`);
  // Root-relative, from this file's directory one level up.
  const rooted = entry.replace(/^\.\//, "../");
  assert(await exists(rooted), `the entry is on disk: ${entry}`);
});

test("the entry re-exports the Counter component", async () => {
  const entry = (await manifest()).exports["."].replace(/^\.\//, "../");
  const source = await file(entry).text();
  assert(
    source.includes("./src/Counter."),
    `the entry re-exports src/Counter: ${source.trim()}`
  );
});

test("Counter takes an initial prop", async () => {
  const candidates = ["../src/Counter.jsx", "../src/Counter.tsx"];
  let source = null;
  for (const path of candidates) {
    if (await exists(path)) {
      source = await file(path).text();
    }
  }
  assert(source !== null, "src/Counter.jsx (or .tsx) exists");
  assert(
    /export default function Counter\(\{\s*initial\s*=\s*0/.test(source),
    "Counter defaults its initial prop to 0"
  );
  assert(
    source.includes('data-testid="counter"'),
    "the button carries its test id"
  );
});
