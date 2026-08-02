// Condition matching in a real package: author order decides, unasserted
// conditions are skipped, arrays fall back, and a null subpath is withdrawn.
import { which as root } from "condkit";              // node skipped → import
import { which as ordered } from "condkit/ordered";   // default written first → default
import { which as fallback } from "condkit/fallback"; // invalid first entry → second
import { which as nested } from "condkit/nested";     // import → nested default

const fail = (m) => { throw new Error("FAIL: " + m); };
if (root !== "esm") fail(`unasserted condition matched: ${root}`);
if (ordered !== "first") fail(`author order ignored: ${ordered}`);
if (fallback !== "fallback") fail(`array fallback: ${fallback}`);
if (nested !== "nested") fail(`nested conditions: ${nested}`);

// `null` withdraws the subpath — and says so.
let withdrawn = null;
try {
  await import("condkit/internal/secret");
} catch (e) {
  withdrawn = e.message;
}
if (withdrawn === null) fail("a null-mapped subpath must not resolve");
if (!withdrawn.includes("withdrew")) fail(`unclear withdrawal error: ${withdrawn}`);

console.log("COND-SUITE-OK");
