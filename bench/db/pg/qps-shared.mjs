// Shared shape for the Postgres QPS benchmark (Bun's shape: 100 rows x
// 100 queries in flight, queries/sec + peak RAM). Each runtime loops 100
// concurrent workers issuing the same 100-row numeric scan; every response
// must hold 100 rows or the run fails. First response is checksummed fully.
export const QPS_QUERY = "SELECT a, b, c FROM bench_num WHERE id <= 100";
export const QPS_ROWS = 100;
export const QPS_WORKERS = 100;
// Bun's work unit: 100,000 queries issued 100 in flight at a time.
export const QPS_TOTAL = 100_000;

export function expectedSum() {
  let a = 0;
  for (let g = 1; g <= QPS_ROWS; g++) a += g * 7;
  return a;
}
