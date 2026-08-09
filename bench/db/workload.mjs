// Shared workload definition for the cross-runtime SQLite benchmark.
//
// The runtimes' SQLite APIs are different shapes — esrun's `runtime:db` is
// async over an op boundary, `node:sqlite` and `bun:sqlite` are synchronous and
// in-process — so unlike `bench/scripts/*.js` there cannot be one script that
// runs everywhere. What *is* shared is this: the schema, the row counts, and
// the checksum each workload must produce, so a runtime cannot win by doing
// less work.
export const ROWS = 50_000;
export const STREAM_ROWS = 200_000;
export const POINTS = 5_000;

export const SCHEMA = `
  CREATE TABLE items (
    id INTEGER PRIMARY KEY,
    a INTEGER NOT NULL,
    b INTEGER NOT NULL,
    c REAL NOT NULL,
    label TEXT NOT NULL,
    body TEXT NOT NULL
  )
`;

export const INSERT = "INSERT INTO items (id, a, b, c, label, body) VALUES (?, ?, ?, ?, ?, ?)";
export const SCAN_NUM = "SELECT a, b, c FROM items";
export const SCAN_TEXT = "SELECT label, body FROM items";
export const POINT = "SELECT a, b FROM items WHERE id = ?";
export const STREAM = "SELECT id, a FROM items";

// Row `i`'s values. Deterministic, so every runtime inserts the same bytes and
// the checksums below are comparable.
export function row(i) {
  return [i, i * 7, i % 1000, i * 1.5, `label-${i % 100}`, `body-${i}-${"x".repeat(40)}`];
}

// The answer each workload must reach. Printed by every script and compared by
// the runner: a mismatch means the runtime skipped work, not that it was fast.
export function expectedNum(rows) {
  let a = 0, b = 0, c = 0;
  for (let i = 0; i < rows; i++) {
    a += i * 7;
    b += i % 1000;
    c += i * 1.5;
  }
  return `${a}/${b}/${c.toFixed(1)}`;
}

export function expectedText(rows) {
  let n = 0;
  for (let i = 0; i < rows; i++) n += `label-${i % 100}`.length + `body-${i}-${"x".repeat(40)}`.length;
  return String(n);
}

export function expectedPoints(points) {
  let sum = 0;
  for (let i = 0; i < points; i++) {
    const id = (i * 7919) % ROWS;
    sum += id * 7 + (id % 1000);
  }
  return String(sum);
}

export function pointId(i) {
  return (i * 7919) % ROWS;
}
