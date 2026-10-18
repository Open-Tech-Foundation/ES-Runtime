// The variable byte integer: MQTT's one length form.
//
// The cases worth pinning are the boundaries where the octet count changes and
// the two ways it can be wrong: a value too large to encode, and bytes that run
// off the end before the continuation bit clears.
import { exit } from "runtime:process";
import {
  decodeVarint,
  encodeVarint,
  MAX_VARINT,
  varintLength,
} from "../../dist/protocol/varint.js";
import { ok, report, throws } from "./assert.mjs";

function bytes(value) {
  return Array.from(encodeVarint(value));
}

ok(bytes(0).join() === "0", "0 is one octet, 0x00");
ok(bytes(127).join() === "127", "127 is one octet, 0x7f");
ok(bytes(128).join() === "128,1", "128 is two octets, 0x80 0x01");
ok(bytes(16383).join() === "255,127", "16383 is two octets, 0xff 0x7f");
ok(bytes(16384).join() === "128,128,1", "16384 is three octets");
ok(bytes(2097151).join() === "255,255,127", "2097151 is three octets");
ok(bytes(2097152).join() === "128,128,128,1", "2097152 is four octets");
ok(bytes(MAX_VARINT).join() === "255,255,255,127", "the maximum is four octets");

ok(varintLength(0) === 1, "length 0 is 1");
ok(varintLength(127) === 1, "length 127 is 1");
ok(varintLength(128) === 2, "length 128 is 2");
ok(varintLength(16383) === 2, "length 16383 is 2");
ok(varintLength(16384) === 3, "length 16384 is 3");
ok(varintLength(2097151) === 3, "length 2097151 is 3");
ok(varintLength(2097152) === 4, "length 2097152 is 4");
ok(varintLength(MAX_VARINT) === 4, "length of the maximum is 4");

for (const value of [0, 1, 127, 128, 255, 16383, 16384, 2097151, 2097152, MAX_VARINT]) {
  const decoded = decodeVarint(encodeVarint(value));
  ok(decoded.value === value, `round-trip ${value}`);
  ok(decoded.length === varintLength(value), `round-trip length ${value}`);
}

// A redundant leading octet is unusual but not wrong: the value is the same.
ok(decodeVarint(new Uint8Array([0x80, 0x00])).value === 0, "a redundant octet still decodes to 0");
ok(decodeVarint(new Uint8Array([0x80, 0x00])).length === 2, "and reports two octets");

await throws(() => encodeVarint(-1), "a negative value is refused");
await throws(() => encodeVarint(MAX_VARINT + 1), "a value past the maximum is refused");
await throws(() => encodeVarint(1.5), "a fractional value is refused");
await throws(() => decodeVarint(new Uint8Array([])), "decoding an empty buffer throws");
await throws(
  () => decodeVarint(new Uint8Array([0x80])),
  "a continuation bit with no next octet throws",
);
await throws(
  () => decodeVarint(new Uint8Array([0xff, 0xff, 0xff, 0xff, 0x7f])),
  "a fifth octet throws",
);

// The decoder is usable mid-stream: it reports how far to advance.
const stream = new Uint8Array([0x81, 0x01, 0x2a]);
const first = decodeVarint(stream, 0);
ok(first.value === 129 && first.length === 2, "the first value spans two octets");
ok(stream[first.length] === 0x2a, "the byte after it is where the field ends");

exit(report("varint") > 0 ? 1 : 0);
