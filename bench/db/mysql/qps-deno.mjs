// MySQL QPS on Deno: mysql2, pool of 100, prepared statements (execute).
import mysql from "mysql2/promise";
import { check, measure, QPS_QUERY } from "./qps-shared.mjs";

const pool = mysql.createPool({ uri: Deno.env.get("MYSQL_URL"), connectionLimit: 100 });
await measure(async () => check((await pool.execute(QPS_QUERY))[0]), Number(Deno.env.get("QPS_WARMUP") ?? 3));
await pool.end();
