import "../dist/index.js";
import { connect, runBackendConformance } from "runtime:db";
import { env } from "runtime:process";
const url = env.PG_URL ?? "postgres://postgres:esrun@127.0.0.1:5433/esrun_test?sslmode=disable";
const report = await runBackendConformance(() => connect(url));
for (const f of report.failures) console.log(`FAIL ${f.name}\n      ${f.error}`);
console.log(`ok=${report.ok} passed=${report.passed} skipped=${report.skipped}`);
