// Shared shape for the MySQL QPS benchmark — the Postgres one's, so the two read
// side by side: 100 rows x 100 queries in flight, 100,000 queries, queries/sec.
// Every response must hold 100 rows or the run fails, and the first is
// checksummed, so a runtime cannot win by doing less.
export const QPS_QUERY = "SELECT a, b, c FROM bench_num WHERE id <= 100";
export const QPS_ROWS = 100;
export const QPS_WORKERS = 100;
export const QPS_TOTAL = 100_000;

export function expectedSum() {
  let a = 0;
  for (let g = 1; g <= QPS_ROWS; g++) a += g * 7;
  return a;
}

/**
 * Warms up for `warmupSeconds` with `QPS_WORKERS` loops, then times
 * `QPS_TOTAL` queries issued `QPS_WORKERS` at a time, and prints the result as
 * the JSON line qps-run.sh reads. `one()` runs the query and returns its rows.
 */
export async function measure(one, warmupSeconds) {
  let sum = 0;
  for (const r of await one()) sum += Number(r.a);
  if (sum !== expectedSum()) throw new Error(`checksum mismatch: ${sum}`);

  const end = Date.now() + warmupSeconds * 1000;
  const workers = [];
  for (let k = 0; k < QPS_WORKERS; k++) {
    workers.push(
      (async () => {
        while (Date.now() < end) await one();
      })(),
    );
  }
  await Promise.all(workers);

  const t0 = Date.now();
  let promises = [];
  for (let i = 0; i < QPS_TOTAL; i++) {
    promises.push(one());
    if (i % QPS_WORKERS === 0 && promises.length > 1) {
      await Promise.all(promises);
      promises = [];
    }
  }
  await Promise.all(promises);
  const seconds = (Date.now() - t0) / 1000;
  console.log(JSON.stringify({ queries: QPS_TOTAL, seconds, qps: Math.round(QPS_TOTAL / seconds) }));
}

export function check(rows) {
  if (rows.length !== QPS_ROWS) throw new Error(`expected ${QPS_ROWS} rows, got ${rows.length}`);
  return rows;
}
