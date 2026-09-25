// MySQL QPS on Node: mysql2, pool of 100, prepared statements (execute).
import mysql from "mysql2/promise";
import { check, measure, QPS_QUERY } from "./qps-shared.mjs";

const pool = mysql.createPool({ uri: process.env.MYSQL_URL, connectionLimit: 100 });
await measure(async () => check((await pool.execute(QPS_QUERY))[0]), Number(process.env.QPS_WARMUP ?? 3));
await pool.end();
