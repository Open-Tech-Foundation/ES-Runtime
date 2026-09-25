// Seed for the MySQL QPS benchmark: bench_num, indexed on id so the queries
// measure drivers rather than table scans. Run with Node (mysql2), once.
import mysql from "mysql2/promise";

const db = await mysql.createConnection({ uri: process.env.MYSQL_URL, multipleStatements: true });
await db.query(`
  DROP TABLE IF EXISTS bench_num;
  CREATE TABLE bench_num (id INT PRIMARY KEY, a INT, b BIGINT, c DOUBLE);
`);
const rows = [];
for (let g = 1; g <= 10_000; g++) rows.push([g, g * 7, g * 1000, g * 1.5]);
await db.query("INSERT INTO bench_num VALUES ?", [rows]);
const [[{ n }]] = await db.query("SELECT COUNT(*) AS n FROM bench_num");
console.log(`seeded bench_num with ${n} rows`);
await db.end();
