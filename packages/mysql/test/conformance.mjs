import { connect, runBackendConformance } from "runtime:db";
import { env } from "runtime:process";
import { driver as mysql } from "../dist/index.js";

const url = env.MYSQL_URL ?? "mysql://root:esrun@127.0.0.1:3307/esrun_test?ssl-mode=DISABLED";
const report = await runBackendConformance(() => connect(url, { driver: mysql }));
for (const f of report.failures) console.log(`FAIL ${f.name}\n      ${f.error}`);
for (const s of report.skips ?? []) console.log(`skip ${s.name}: ${s.reason}`);
console.log(`ok=${report.ok} passed=${report.passed} skipped=${report.skipped}`);
if (!report.ok) throw new Error("conformance failed");
