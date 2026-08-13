// Array literals and parameter encoding — the two places a driver quietly
// corrupts data rather than failing.

import { exit } from "runtime:process";
import { decoderFor, encodeParam, OID, parseArray } from "../../dist/protocol/values.js";
import { is, report } from "./assert.mjs";

const text = (t) => t;
const p = (literal) => JSON.stringify(parseArray(literal, text));

is(p("{}"), "[]", "empty array");
is(p("{1,2,3}"), '["1","2","3"]', "flat array");
is(p("{NULL,1}"), '[null,"1"]', "an unquoted NULL is absence");
// The distinction that matters: quoting is what separates the value from the
// keyword, and losing it turns data into absence.
is(p('{"NULL",1}'), '["NULL","1"]', "a quoted NULL is four characters");
is(p('{"a,b"}'), '["a,b"]', "the delimiter inside a quoted element");
is(p('{"a}b"}'), '["a}b"]', "a closing brace inside a quoted element");
is(p('{"say \\"hi\\""}'), '["say \\"hi\\""]', "an escaped quote");
is(p('{"back\\\\slash"}'), '["back\\\\slash"]', "an escaped backslash");
is(p("{{1,2},{3,4}}"), '[["1","2"],["3","4"]]', "nesting");
is(p("[2:4]={7,8,9}"), '["7","8","9"]', "an explicit lower bound is skipped");
is(p("{}"), "[]", "empty again after the bound form");

// The element decoder is applied, so an int8[] is numbers rather than strings.
const decode = decoderFor(1016); // _int8
const bytes = new TextEncoder().encode("{1,9007199254740993}");
const view = new DataView(bytes.buffer);
const decoded = decode(bytes, view, 0, bytes.length);
is(typeof decoded[0], "number", "a small int8 element is a number");
is(typeof decoded[1], "bigint", "an int8 element too large for a number is a bigint");

// Parameters.
const enc = (v) => {
  const bytes = encodeParam(v);
  // `encodeParam` answers null for SQL NULL, which is not the same as encoding
  // the four letters — the wire carries a length of -1 and no payload.
  return bytes === null ? "NULL" : new TextDecoder().decode(bytes);
};
is(enc(null), "NULL", "null binds as SQL NULL");
is(enc(undefined), "NULL", "undefined binds as SQL NULL");
is(enc(42), "42", "a number");
is(enc(1.5), "1.5", "a float");
is(enc(9007199254740993n), "9007199254740993", "a bigint keeps its digits");
is(enc(true), "t", "true");
is(enc(false), "f", "false");
is(enc(Number.NaN), "NaN", "NaN is spelled the way the server spells it");
is(enc(Number.POSITIVE_INFINITY), "Infinity", "infinity");
is(enc(new Uint8Array([0, 255])), "\\x00ff", "bytes bind as hex bytea");
is(enc([1, 2]), '{"1","2"}', "an array binds as an array literal");
is(enc([null, 1]), '{NULL,"1"}', "a null element binds as the keyword");
is(enc({ a: 1 }), '{"a":1}', "an object binds as JSON");
is(
  enc(new Date("2026-01-02T03:04:05.000Z")),
  "2026-01-02T03:04:05.000Z",
  "a Date binds as ISO-8601",
);

// numeric stays a string: it is arbitrary precision by definition, and a double
// is the one representation guaranteed to lose it.
const numeric = decoderFor(OID.numeric);
const nbytes = new TextEncoder().encode("0.1000000000000000000001");
is(
  numeric(nbytes, new DataView(nbytes.buffer), 0, nbytes.length),
  "0.1000000000000000000001",
  "numeric is not rounded through a double",
);

if (report("values") > 0) exit(1);
