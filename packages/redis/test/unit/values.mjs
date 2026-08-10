// Replies becoming values, and replies becoming rows.
//
// The two conversions are separate functions, so the thing worth testing is
// that they agree: a value must not change type depending on whether it came
// through the client API or through `runtime:db`'s row decoder.
import { exit } from "runtime:process";
import { is, ok, report } from "./assert.mjs";
import { rowsOf as rowsOfReply, shapeOf, toValue } from "../../dist/protocol/values.js";

const encoder = new TextEncoder();

const bulk = (text) => ({ kind: "string", value: text, bytes: encoder.encode(text) });
const int = (value) => ({ kind: "integer", value: BigInt(value) });
const NULL = { kind: "null" };

/** Reads a reply through the row path, the way a caller would receive it. */
async function rowsOf(reply, options = {}) {
  const out = [];
  for await (const row of rowsOfReply(reply, options)) out.push(row.toObject());
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

is(await rowsOf({ kind: "array", value: [bulk("a"), bulk("b")] }), [{ value: "a" }, { value: "b" }],
  "an aggregate is one row per element");
is(await rowsOf(bulk("only")), [{ value: "only" }], "a scalar is one row");
is(await rowsOf(NULL), [], "a null reply is no rows — GET on a missing key answers no row");
is(await rowsOf({ kind: "array", value: [] }), [], "an empty array is also no rows");
is(
  await rowsOf({ kind: "map", value: [[bulk("f"), bulk("v")]] }),
  [{ field: "f", value: "v" }],
  "a map is two columns",
);
is(
  await rowsOf({ kind: "array", value: [bulk("a"), NULL, bulk("c")] }),
  [{ value: "a" }, { value: null }, { value: "c" }],
  "a null element is a null cell, and does not shorten the result",
);

// The heterogeneous case, which is the one the tagged encoding exists for: a
// Redis array need not agree with itself, so the type travels per value.
is(
  await rowsOf({ kind: "array", value: [int(1), bulk("two"), { kind: "double", value: 3.5 }] }),
  [{ value: 1 }, { value: "two" }, { value: 3.5 }],
  "one array, three types, each decoded as itself",
);

// The agreement that matters.
ok(
  (await rowsOf({ kind: "array", value: [int(9007199254740993n)] }))[0].value === 9007199254740993n,
  "the row path narrows integers exactly as toValue does",
);

{
  const rows = await rowsOf({ kind: "array", value: [bulk("bytes")] }, { binary: true });
  ok(rows[0].value instanceof Uint8Array, "binary mode reaches the row path too");
}

// -- batching ---------------------------------------------------------------

{
  // A reply larger than one batch has to come back in several, and every row
  // has to appear exactly once across them, in order.
  const reply = { kind: "array", value: Array.from({ length: 3000 }, (_, i) => bulk(`item-${i}`)) };
  const seen = [];
  for await (const row of rowsOfReply(reply, {})) seen.push(row.value);
  is(seen.length, 3000, "every row came back");
  is(seen[0], "item-0", "in order, from the first");
  is(seen[2999], "item-2999", "to the last");
  is(new Set(seen).size, 3000, "and none of them twice");
}

{
  // The reason batching survived the move to records: a caller who stops after
  // three rows must not have paid for three thousand. Counted through the
  // reply itself — a cell that reports being read.
  let touched = 0;
  const counting = {
    kind: "array",
    value: Array.from({ length: 3000 }, (_, i) => {
      const cell = bulk(`item-${i}`);
      return {
        get kind() {
          touched++;
          return cell.kind;
        },
        get value() {
          return cell.value;
        },
        get bytes() {
          return cell.bytes;
        },
      };
    }),
  };
  const taken = [];
  for await (const row of rowsOfReply(counting, {})) {
    taken.push(row.value);
    if (taken.length === 3) break;
  }
  is(taken.length, 3, "the caller took three rows");
  ok(touched < 3000, `and paid for one batch rather than the whole reply (${touched} cells read)`);
}

{
  // A single enormous row is still one row. The byte budget this used to be
  // measured against is gone with the encoding — records are counted, and a
  // count cannot be overrun by one wide value.
  const reply = { kind: "array", value: [bulk("x".repeat(4096))] };
  const rows = await rowsOf(reply);
  is(rows.length, 1, "a row far larger than any budget comes back whole");
  is(rows[0].value.length, 4096, "with every byte of it");
}

if (report("values") > 0) exit(1);
