// Seed for the DB-backed endpoint benchmark: the shared bench_num table
// (200k rows via generate_series — server-side, seconds).
import postgres from "postgres";
import * as w from "./workload.mjs";

const sql = postgres(process.env.PG_URL, { fetch_types: false });
await sql.unsafe(w.SCHEMA);
// The endpoint and QPS benches filter by id: without this every query
// seq-scans all 200k rows and the number measures Postgres, not the driver.
await sql.unsafe("CREATE INDEX IF NOT EXISTS bench_num_id_idx ON bench_num (id)");
const [{ count }] = await sql`SELECT count(*)::int AS count FROM bench_num`;
console.log(`seeded bench_num with ${count} rows`);
await sql.end();
