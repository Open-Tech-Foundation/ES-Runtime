// Flattens a yaml-test-suite data checkout into cases.json, the one file the
// Rust tests read. Run from the repository root:
//
//   esdev crates/runtime/yaml-test-suite/generate.js "$PWD/target/yaml-test-suite"
//
// after cloning the data tag README.md names into target/yaml-test-suite. The
// checkout must sit inside the repository: the runtime jails file access to it.
import { file, readDir, write } from "runtime:fs";
import { args } from "runtime:process";

const root = args[0];
if (!root) throw new Error("usage: generate.js <yaml-test-suite data checkout>");

const read = async (path) => {
  try {
    return await file(path).text();
  } catch {
    return null;
  }
};

const cases = [];
const visit = async (dir, id) => {
  const yaml = await read(`${dir}/in.yaml`);
  if (yaml === null) return;
  cases.push({
    id,
    name: ((await read(`${dir}/===`)) ?? "").trim(),
    yaml,
    events: await read(`${dir}/test.event`),
    json: await read(`${dir}/in.json`),
    error: (await read(`${dir}/error`)) !== null,
  });
};

const names = [];
names.push(...(await readDir(root)));
for (const entry of names.sort((a, b) => (a.name < b.name ? -1 : 1))) {
  if (!entry.isDir || entry.name.startsWith(".") || entry.name === "name" || entry.name === "tags") continue;
  const dir = `${root}/${entry.name}`;
  if (await read(`${dir}/in.yaml`) !== null) {
    await visit(dir, entry.name);
    continue;
  }
  const subs = [];
  for (const sub of await readDir(dir)) subs.push(sub.name);
  for (const sub of subs.sort()) await visit(`${dir}/${sub}`, `${entry.name}/${sub}`);
}

await write(new URL("cases.json", import.meta.url).pathname, JSON.stringify(cases, null, 1) + "\n");
console.log(`${cases.length} cases`);
