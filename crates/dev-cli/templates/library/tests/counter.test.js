import { assert, test } from "runtime:test";
import { exists, file } from "runtime:fs";

// Paths are relative to this file. Rendering the component needs the OTF
// compiler, so this checks what consumers import instead.
test("the package entry exports Counter", async () => {
  const manifest = JSON.parse(await file("../package.json").text());
  const entry = manifest.exports["."].replace(/^\.\//, "../");
  assert(await exists(entry), `${entry} exists`);
  assert((await file(entry).text()).includes("./src/Counter."), "it re-exports src/Counter");
});
