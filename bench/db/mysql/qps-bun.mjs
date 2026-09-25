// MySQL QPS on Bun: the built-in Bun.SQL MySQL client, pool of 100.
import { SQL } from "bun";
import { check, measure } from "./qps-shared.mjs";

// Key retrieval allowed, as mysql2 does by default; Bun asks for it to be said.
const sql = new SQL(process.env.MYSQL_URL, { max: 100, allowPublicKeyRetrieval: true });
await measure(async () => check(await sql`SELECT a, b, c FROM bench_num WHERE id <= 100`), Number(process.env.QPS_WARMUP ?? 3));
await sql.close();
