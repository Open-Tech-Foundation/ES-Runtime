// The kit's conformance suite, against a backend that does not speak SQL.
//
// Most of the suite is written in SQL DDL and DML, and Redis can express none
// of it. The point of running it anyway is that the suite has to *say* so:
// a check a backend cannot express is skipped with a reason, not failed, and
// not silently counted as a pass. Before Redis there was no backend to prove
// that distinction existed.

import { connect, runBackendConformance } from "runtime:db";
import { env, exit } from "runtime:process";

import { driver as redis } from "../dist/index.js";
import { is, ok, report } from "./unit/assert.mjs";

const url = env.REDIS_URL ?? "redis://127.0.0.1:6379";

const result = await runBackendConformance(() => connect(url, { driver: redis }));

for (const check of result.results) {
  if (check.ok === false) console.log(`  FAIL ${check.name}\n       ${check.error}`);
}

ok(result.ok, `the suite passed (${result.passed} passed, ${result.skipped} skipped)`);
is(result.failures.length, 0, "nothing failed");

// The checks that hold whatever form a backend takes are the ones that must
// actually run. If these were skipped too, the report would be vacuous.
ok(result.passed >= 2, `${result.passed} form-agnostic checks ran`);
const ran = result.results.filter((r) => r.ok === true).map((r) => r.name);
ok(
  ran.includes("a closed connection refuses work rather than hanging"),
  "the closed-connection check ran against Redis",
);
ok(
  ran.includes("the query form this backend does not take is refused by name"),
  "and the query-form check, which asked for SQL rather than for an AST",
);

// Every skip carries a reason. A count with no explanation is how a driver
// author concludes they passed something they never ran.
const skipped = result.results.filter((r) => r.skipped);
ok(skipped.length > 0, `${skipped.length} SQL checks were skipped`);
ok(
  skipped.every((r) => typeof r.reason === "string" && r.reason.length > 0),
  "and every one of them said why",
);

if (report("conformance") > 0) exit(1);
