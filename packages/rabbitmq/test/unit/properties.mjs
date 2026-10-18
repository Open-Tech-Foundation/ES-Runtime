// The content header, with no broker anywhere near it.
//
// The cases worth pinning are the ones where the flag word and the values have
// to agree: a property that is absent must cost nothing and shift every
// position after it, which is the part a round-trip alone would not catch.
import { exit } from "runtime:process";
import { decodeContentHeader, encodeContentHeader } from "../../dist/protocol/properties.js";
import { ByteWriter } from "../../dist/protocol/wire.js";
import { is, ok, report } from "./assert.mjs";

function same(a, b) {
  if (a === b) return true;
  if (typeof a === "bigint" || typeof b === "bigint") return false;
  if (a instanceof Uint8Array && b instanceof Uint8Array) {
    return a.length === b.length && a.every((byte, i) => byte === b[i]);
  }
  if (a instanceof Map && b instanceof Map) {
    if (a.size !== b.size) return false;
    for (const [key, value] of a) {
      if (!b.has(key) || !same(value, b.get(key))) return false;
    }
    return true;
  }
  if (a && b && typeof a === "object" && typeof b === "object") {
    const keys = Object.keys(a);
    if (keys.length !== Object.keys(b).length) return false;
    return keys.every((key) => same(a[key], b[key]));
  }
  return false;
}

function header(properties, bodySize = 0n) {
  return { classId: 60, weight: 0, bodySize, properties };
}

function roundTrip(properties, bodySize = 0n) {
  return decodeContentHeader(encodeContentHeader(header(properties, bodySize)));
}

// -- the whole bag ----------------------------------------------------------

const EVERYTHING = {
  contentType: "application/json",
  contentEncoding: "gzip",
  headers: new Map([
    ["x-retry", { type: "int32", value: 3 }],
    ["x-tags", { type: "array", value: [{ type: "string", value: "a" }] }],
  ]),
  deliveryMode: 2,
  priority: 9,
  correlationId: "c-1",
  replyTo: "amq.rabbitmq.reply-to",
  expiration: "60000",
  messageId: "m-1",
  timestamp: 1700000000n,
  type: "task",
  userId: "guest",
  appId: "esrun",
  clusterId: "c",
};

{
  const decoded = roundTrip(EVERYTHING, 42n);
  is(decoded.classId, 60, "the class is basic");
  is(decoded.weight, 0, "the weight is zero");
  ok(decoded.bodySize === 42n, "the body size is exact");
  ok(same(decoded.properties, EVERYTHING), "every property survives");
}

// -- the flags --------------------------------------------------------------

// The flag word sits after the class, weight, and body size: byte 12.
function flagsOf(properties) {
  const bytes = encodeContentHeader(header(properties));
  return (bytes[12] << 8) | bytes[13];
}

const FLAG_BITS = [
  ["contentType", 15],
  ["deliveryMode", 12],
  ["correlationId", 10],
  ["replyTo", 9],
  ["timestamp", 6],
  ["clusterId", 2],
];

for (const [name, bit] of FLAG_BITS) {
  const value = {
    contentType: "text/plain",
    deliveryMode: 2,
    correlationId: "c",
    replyTo: "r",
    timestamp: 1n,
    clusterId: "c",
  }[name];
  is(flagsOf({ [name]: value }), 1 << bit, `${name} is flag bit ${bit}`);
}

// Absence costs nothing and shifts nothing: a lone correlation-id is that bit
// and no other, and decoding gives no delivery-mode it never sent.
{
  const decoded = roundTrip({ correlationId: "c" });
  is(decoded.properties.correlationId, "c", "the one property present comes back");
  ok(decoded.properties.deliveryMode === undefined, "an absent property stays absent");
  ok(decoded.properties.headers === undefined, "an absent table stays absent");
}

{
  const decoded = roundTrip({});
  ok(same(decoded.properties, {}), "no properties round-trips to none");
}

// An empty table is present, not absent — the difference between "no headers"
// and "headers that are empty".
{
  const decoded = roundTrip({ headers: new Map() });
  ok(
    decoded.properties.headers instanceof Map && decoded.properties.headers.size === 0,
    "an empty table is preserved",
  );
}

{
  const withHeaders = encodeContentHeader(header({ headers: new Map() }));
  const withNothing = encodeContentHeader(header({}));
  ok(withHeaders.length > withNothing.length, "a present empty table costs bytes");
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

refuses("encoding a header for another class is refused", () =>
  encodeContentHeader({ classId: 40, weight: 0, bodySize: 0n, properties: {} }),
);

refuses("encoding a header with a weight is refused", () =>
  encodeContentHeader({ classId: 60, weight: 1, bodySize: 0n, properties: {} }),
);

refuses("encoding a delivery mode other than 1 or 2 is refused", () =>
  encodeContentHeader(header({ deliveryMode: 3 })),
);

// Hand-built, since the encoder will never emit one.
function payload(classId, weight, flags) {
  const w = new ByteWriter(16);
  w.u16(classId).u16(weight).u64(0n).u16(flags);
  return w.finish();
}

refuses("decoding a header for another class is refused", () =>
  decodeContentHeader(payload(40, 0, 0)),
);
refuses("decoding a header with a weight is refused", () => decodeContentHeader(payload(60, 1, 0)));

// Bit 0 says a second flag word follows. This client would not know what to do
// with one, so reading on as if it had not seen it would misread every value.
refuses("decoding a continuation flag is refused", () =>
  decodeContentHeader(payload(60, 0, 0x0001)),
);

// Bytes after the properties mean the two sides disagree about what was
// present, not that the extra is padding.
{
  const bytes = encodeContentHeader(header({ contentType: "text/plain" }));
  const extra = new Uint8Array(bytes.length + 1);
  extra.set(bytes);
  refuses("decoding trailing bytes is refused", () => decodeContentHeader(extra));
}

if (report("properties") > 0) exit(1);
