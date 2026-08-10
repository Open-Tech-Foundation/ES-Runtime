// Shared workload for the PostgreSQL driver comparison.
//
// The acceptance criteria D56 set for this path: numeric-heavy rows should win
// by a wide margin, text-heavy rows should at least tie (both sides pay
// TextDecoder, and only the JS engine can make a JS string), a small query must
// not regress meaningfully, and streaming must not grow memory with the result.
export const ROWS = 10_000;
export const STREAM_ROWS = 200_000;
export const SMALL_REPEATS = 500;

export const SCHEMA = `
  DROP TABLE IF EXISTS bench_num;
  DROP TABLE IF EXISTS bench_text;
  CREATE TABLE bench_num (id int, a int, b bigint, c float8, d float8, e int);
  CREATE TABLE bench_text (id int, s1 text, s2 text, s3 text);
  INSERT INTO bench_num
    SELECT g, g * 7, g::bigint * 1000, g * 1.5, g / 3.0, g % 97
    FROM generate_series(1, ${STREAM_ROWS}) g;
  INSERT INTO bench_text
    SELECT g, 'label-' || (g % 100), repeat('x', 60), md5(g::text)
    FROM generate_series(1, ${ROWS}) g;
`;

export const SCAN_NUM = `SELECT a, b, c, d, e FROM bench_num WHERE id <= ${ROWS}`;
export const SCAN_TEXT = `SELECT s1, s2, s3 FROM bench_text`;
export const SMALL = "SELECT a, b, c FROM bench_num WHERE id <= 10";
export const STREAM = "SELECT id, a FROM bench_num";

/** The answers, so a runtime cannot look fast by doing less. */
export function expectedNum() {
  let a = 0, b = 0n, c = 0, d = 0, e = 0;
  for (let g = 1; g <= ROWS; g++) {
    a += g * 7;
    b += BigInt(g) * 1000n;
    c += g * 1.5;
    d += g / 3.0;
    e += g % 97;
  }
  return `${a}/${b}/${c.toFixed(1)}/${d.toFixed(2)}/${e}`;
}

export function expectedText() {
  let n = 0;
  for (let g = 1; g <= ROWS; g++) n += `label-${g % 100}`.length + 60 + 32;
  return String(n);
}
