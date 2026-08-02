// `imports` (#private specifiers) and package self-reference, both defined by
// tests/fixtures/package.json.
import { PI } from "#local";                    // #specifier → a path in this package
import { feature } from "#feat/one";            // #specifier → subpath pattern
import { hi } from "#greeter";                  // #specifier → another package
import { add } from "esrun-fixtures/exporter";  // self-reference through "exports"

const fail = (m) => { throw new Error("FAIL: " + m); };
if (PI !== 3.14159) fail("#local");
if (feature !== "feat-one") fail("#feat pattern");
if (hi("x") !== "hi x from greeter") fail("#specifier naming a package");
if (add(1, 2) !== 3) fail("self-reference");

// An undeclared #specifier fails clearly rather than being looked up on disk.
let err = null;
try {
  await import("#nope");
} catch (e) {
  err = e.message;
}
if (err === null || !err.includes("does not define")) fail(`unclear error: ${err}`);

console.log("PRIVATE-SUITE-OK");
