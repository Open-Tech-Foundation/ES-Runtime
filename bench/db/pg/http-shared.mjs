// Shared shape for the DB-backed endpoint benchmark: GET /item?id=N reads
// one row from bench_num and answers JSON. Each runtime gets its own server
// file (its own HTTP surface and its own Postgres driver), but the query,
// the row, and the bytes are identical, so a runtime cannot win by doing less.
//
// Seed once: PG_URL=... node http-seed.mjs
// Serve:     BENCH_PORT=... PG_URL=... <runtime> http-<runtime>.mjs
import * as w from "./workload.mjs";

export const POINT = "SELECT a, b, c FROM bench_num WHERE id = $1";
export const POINT_ID = 100000;

export function body(row) {
  // bigint arrives as BigInt on some drivers: stringify it the same way
  // everywhere so the bytes match.
  return JSON.stringify({ id: POINT_ID, a: row.a, b: String(row.b), c: row.c });
}

export function checkRow(row) {
  const i = POINT_ID;
  if (row.a !== i * 7 || BigInt(row.b) !== BigInt(i) * 1000n || row.c !== i * 1.5) {
    throw new Error(`unexpected row: ${JSON.stringify(row)}`);
  }
}

export { w };
