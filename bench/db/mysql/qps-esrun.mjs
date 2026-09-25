// MySQL QPS on esrun: @opentf/esrun-mysql, pool of 100.
import { connect } from "runtime:db";
import { env } from "runtime:process";
import { driver as mysql } from "./.driver/index.js";
import { check, measure, QPS_QUERY } from "./qps-shared.mjs";

// No TLS and key retrieval allowed — the terms every other runtime here runs on
// by default, stated because this driver's defaults are stricter.
const db = await connect(env.MYSQL_URL, {
  driver: mysql,
  pool: { max: 100 },
  sslmode: "disable",
  allowPublicKeyRetrieval: true,
});
await measure(async () => check(await (await db.query(QPS_QUERY)).toArray()), Number(env.QPS_WARMUP ?? 3));
await db.close();
