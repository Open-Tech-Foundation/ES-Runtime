// Method identities and their arguments, with no broker anywhere near it.
//
// The table is transcribed from the specification, so the test that matters
// most is the tedious one: every method, every field, encoded and decoded back
// to what it started as. Beside that sit the two things a table can still get
// wrong in a way a round-trip would not catch — where a run of bits begins, and
// whether a field's default is the one the wire expects.
import { exit } from "runtime:process";
import {
  ClassId,
  decodeMethod,
  encodeMethod,
  METHODS,
  MethodId,
  methodDefinition,
  methodName,
  readMethodKey,
  writeMethodKey,
} from "../../dist/protocol/methods.js";
import { ByteReader, ByteWriter } from "../../dist/protocol/wire.js";
import { is, ok, report } from "./assert.mjs";

function same(a, b) {
  if (a === b) return true;
  if (typeof a === "bigint" || typeof b === "bigint") return false;
  if (a instanceof Map && b instanceof Map) {
    if (a.size !== b.size) return false;
    for (const [key, value] of a) {
      if (!b.has(key) || !same(value, b.get(key))) return false;
    }
    return true;
  }
  if (Array.isArray(a) && Array.isArray(b)) {
    return a.length === b.length && a.every((item, i) => same(item, b[i]));
  }
  if (a && b && typeof a === "object" && typeof b === "object") {
    const keys = Object.keys(a);
    if (keys.length !== Object.keys(b).length) return false;
    return keys.every((key) => same(a[key], b[key]));
  }
  return false;
}

// -- the identity of a method -----------------------------------------------

is(
  methodName(ClassId.connection, MethodId.connection.start),
  "connection.start",
  "a start method is named",
);
is(methodName(ClassId.basic, MethodId.basic.publish), "basic.publish", "a publish method is named");
is(
  methodName(ClassId.confirm, MethodId.confirm.select),
  "confirm.select",
  "a confirm method is named",
);
is(methodName(ClassId.queue, MethodId.queue.unbindOk), "queue.unbindOk", "an unbind-ok is named");
is(methodName(61, 10), "61:10", "an unknown pair reports its numbers");

{
  const w = new ByteWriter(8);
  writeMethodKey(w, { classId: 60, methodId: 40 });
  const r = new ByteReader(w.finish());
  const key = readMethodKey(r);
  is(key.classId, 60, "the class id round-trips");
  is(key.methodId, 40, "the method id round-trips");
  ok(r.done, "a key is exactly four bytes");
}

{
  let threw = false;
  try {
    writeMethodKey(new ByteWriter(8), { classId: 65536, methodId: 0 });
  } catch {
    threw = true;
  }
  ok(threw, "a class id past 65535 is refused");
}

// -- every method, every field ---------------------------------------------

// A value of the right shape for each domain. Bits alternate so both states are
// exercised inside the runs they share an octet with.
function sample(field, index) {
  switch (field.domain) {
    case "octet":
      return 1;
    case "short":
      return 2;
    case "long":
      return 3;
    case "longlong":
      return 4n;
    case "shortstr":
      return "s";
    case "longstr":
      return "l";
    case "table":
      return new Map([["k", { type: "int16", value: index }]]);
    case "bit":
      return index % 2 === 0;
  }
}

let bitRuns = 0;
for (const definition of METHODS) {
  const args = {};
  let index = 0;
  for (const field of definition.fields) {
    args[field.name] = sample(field, index);
    if (field.domain === "bit") index += 1;
  }
  const decoded = decodeMethod(encodeMethod(definition.name, args));
  is(decoded.name, definition.name, `${definition.name} decodes to itself`);
  is(decoded.classId, definition.classId, `${definition.name} keeps its class`);
  is(decoded.methodId, definition.methodId, `${definition.name} keeps its method`);
  ok(same(decoded.args, args), `${definition.name} keeps every argument`);
  ok(decoded.hasContent === (definition.hasContent === true), `${definition.name} content flag`);
  bitRuns += definition.fields.filter((field) => field.domain === "bit").length;
}
ok(bitRuns > 0, "some methods have bits, or the packing below proves nothing");

// -- defaults ---------------------------------------------------------------

// A bit is off and a table is empty unless said otherwise, and the reserved
// fields are still on the wire with the values the specification fixes.
{
  const decoded = decodeMethod(encodeMethod("queue.declare", {}));
  is(decoded.args.queue, "", "an unnamed queue is an empty name");
  is(decoded.args.passive, false, "a passive declare defaults to off");
  is(decoded.args.noWait, false, "a no-wait declare defaults to off");
  ok(same(decoded.args.arguments, new Map()), "a declare with no arguments sends an empty table");
}

{
  const decoded = decodeMethod(encodeMethod("connection.open", { virtualHost: "/" }));
  is(decoded.args.reserved1, "", "the reserved short string is sent empty");
  is(decoded.args.reserved2, false, "the reserved bit is sent off");
}

// A publish is the one that would be tiresome without defaults: mandatory and
// immediate are the specification's flags, and false is what almost every
// publisher means.
{
  const decoded = decodeMethod(encodeMethod("basic.publish", { exchange: "", routingKey: "k" }));
  is(decoded.args.mandatory, false, "a publish is not mandatory by default");
  is(decoded.args.immediate, false, "a publish is not immediate by default");
  is(decoded.args.reserved1, 0, "the reserved short is sent zero");
}

// -- where a run of bits begins ---------------------------------------------

// `basic.consume`'s four bits share one octet, least-significant first, and the
// octet sits after the two short strings. Byte 9 is that octet:
// key(4) + reserved(2) + "q"(2) + consumer-tag(1).
{
  const payload = encodeMethod("basic.consume", { queue: "q", noAck: true });
  is(payload[9], 0b0010, "no-ack is the second bit");
}
{
  const payload = encodeMethod("basic.consume", { queue: "q", noWait: true, exclusive: true });
  is(payload[9], 0b1100, "no-wait and exclusive are the third and fourth bits");
}
{
  const payload = encodeMethod("basic.consume", { queue: "q" });
  is(payload[9], 0, "a run of false bits still costs its octet");
  is(payload.length, 14, "and the empty argument table costs four more");
}

// -- the refusals ----------------------------------------------------------

function refuses(what, fn) {
  let threw = false;
  try {
    fn();
  } catch {
    threw = true;
  }
  ok(threw, what);
}

refuses("encoding a method that does not exist is refused", () =>
  encodeMethod("basic.explode", {}),
);
refuses("looking up a method that does not exist is refused", () =>
  methodDefinition("basic.explode"),
);

{
  // 99:99 is not a method anyone defines.
  const bytes = new Uint8Array([0x00, 0x63, 0x00, 0x63]);
  refuses("decoding an unknown method is refused", () => decodeMethod(bytes));
}

refuses("omitting a required argument is refused", () => encodeMethod("basic.qos", {}));
refuses("a string where a number belongs is refused", () =>
  encodeMethod("basic.publish", { exchange: 5, routingKey: "k" }),
);
refuses("a number where a string belongs is refused", () =>
  encodeMethod("basic.publish", { exchange: "", routingKey: 5 }),
);
refuses("a non-boolean bit is refused", () =>
  encodeMethod("basic.ack", { deliveryTag: 1n, multiple: "yes" }),
);
refuses("a delivery tag past the safe integer range is refused", () =>
  encodeMethod("basic.ack", { deliveryTag: 2 ** 53 }),
);
refuses("an out-of-range short is refused", () =>
  encodeMethod("basic.qos", { prefetchSize: 0, prefetchCount: 65536 }),
);
refuses("a field table that is not a table is refused", () =>
  encodeMethod("queue.declare", { arguments: {} }),
);

{
  const bytes = new Uint8Array([...encodeMethod("connection.close-ok"), 0x00]);
  refuses("trailing bytes after a method are refused", () => decodeMethod(bytes));
}

if (report("methods") > 0) exit(1);
