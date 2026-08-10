// Replies becoming values, and replies becoming rows.
//
// The two conversions are separate functions, so the thing worth testing is
// that they agree: a value must not change type depending on whether it came
// through the client API or through `runtime:db`'s row decoder.
import { exit } from "runtime:process";
import { decodeBatch, defineRowShape } from "runtime:db";

import { is, ok, report } from "./assert.mjs";
import { shapeOf, toValue, writeRows } from "../../dist/protocol/values.js";

const encoder = new TextEncoder();

const bulk = (text) => ({ kind: "string", value: text, bytes: encoder.encode(text) });
const int = (value) => ({ kind: "integer", value: BigInt(value) });
const NULL = { kind: "null" };

/** Decodes a reply through the row path, the way a caller would receive it. */
function rowsOf(reply, options = {}) {
  const { columns, rows: total } = shapeOf(reply);
  const shape = defineRowShape(columns);
  const out = [];
  let at = 0;
  while (at < total) {
    const batch = writeRows(reply, at, total, 64 * 1024, options);
    ok(batch.rows > 0, "a batch that made no progress would loop forever");
    for (const row of decodeBatch(batch.bytes, shape, batch.rows)) out.push(row.toObject());
    at += batch.rows;
  }
  return out;
}

// -- toValue ----------------------------------------------------------------

is(toValue(bulk("hello")), "hello", "a bulk string is text by default");
ok(toValue(bulk("hello"), { binary: true }) instanceof Uint8Array, "and bytes in binary mode");
is(toValue(int(42)), 42, "an integer that fits a number is a number");
ok(toValue(int(9007199254740993n)) === 9007199254740993n, "one that does not stays a bigint");
is(toValue(NULL), null, "null is null");
is(toValue({ kind: "boolean", value: true }), true, "a boolean");
is(toValue({ kind: "double", value: 1.5 }), 1.5, "a double");
is(
  toValue({ kind: "map", value: [[bulk("a"), int(1)]] }),
  { a: 1 },
  "a map is a plain object, which is what HGETALL is for",
);
is(toValue({ kind: "array", value: [bulk("a"), int(2)] }), ["a", 2], "an array keeps its element types");

// An error nested in a reply is still an error. Turning it into a string would
// hand the caller a row that reads like data.
{
  let threw = false;
  try {
    toValue({ kind: "array", value: [{ kind: "error", value: { prefix: "ERR", message: "ERR bad" } }] });
  } catch {
    threw = true;
  }
  ok(threw, "an error nested in an array is thrown, not stringified");
}

// -- the row layout ---------------------------------------------------------

is(rowsOf({ kind: "array", value: [bulk("a"), bulk("b")] }), [{ value: "a" }, { value: "b" }],
  "an aggregate is one row per element");
is(rowsOf(bulk("only")), [{ value: "only" }], "a scalar is one row");
is(rowsOf(NULL), [], "a null reply is no rows — GET on a missing key answers no row");
is(rowsOf({ kind: "array", value: [] }), [], "an empty array is also no rows");
is(
  rowsOf({ kind: "map", value: [[bulk("f"), bulk("v")]] }),
  [{ field: "f", value: "v" }],
  "a map is two columns",
);
is(
  rowsOf({ kind: "array", value: [bulk("a"), NULL, bulk("c")] }),
  [{ value: "a" }, { value: null }, { value: "c" }],
  "a null element is a null cell, and does not shorten the result",
);

// The heterogeneous case, which is the one the tagged encoding exists for: a
// Redis array need not agree with itself, so the type travels per value.
is(
  rowsOf({ kind: "array", value: [int(1), bulk("two"), { kind: "double", value: 3.5 }] }),
  [{ value: 1 }, { value: "two" }, { value: 3.5 }],
  "one array, three types, each decoded as itself",
);

// The agreement that matters.
ok(
  rowsOf({ kind: "array", value: [int(9007199254740993n)] })[0].value === 9007199254740993n,
  "the row path narrows integers exactly as toValue does",
);

{
  const rows = rowsOf({ kind: "array", value: [bulk("bytes")] }, { binary: true });
  ok(rows[0].value instanceof Uint8Array, "binary mode reaches the row path too");
}

// -- batching ---------------------------------------------------------------

{
  // A reply larger than one batch has to come back in several, and every row
  // has to appear exactly once across them.
  const reply = { kind: "array", value: Array.from({ length: 500 }, (_, i) => bulk(`item-${i}`)) };
  const { columns, rows: total } = shapeOf(reply);
  const shape = defineRowShape(columns);
  const seen = [];
  let at = 0;
  let batches = 0;
  while (at < total) {
    const batch = writeRows(reply, at, total, 512, {});
    batches++;
    for (const row of decodeBatch(batch.bytes, shape, batch.rows)) seen.push(row.value);
    at += batch.rows;
  }
  ok(batches > 1, `500 rows at 512 bytes took ${batches} batches`);
  is(seen.length, 500, "every row came back");
  is(seen[0], "item-0", "in order, from the first");
  is(seen[499], "item-499", "to the last");
  is(new Set(seen).size, 500, "and none of them twice");
}

{
  // A row bigger than the whole budget still has to be emitted, or the reader
  // would make no progress and loop forever.
  const reply = { kind: "array", value: [bulk("x".repeat(4096))] };
  const batch = writeRows(reply, 0, 1, 16, {});
  is(batch.rows, 1, "a row larger than the batch budget is emitted anyway");
  ok(batch.done, "and the result is complete");
}

if (report("values") > 0) exit(1);
