// The wire primitives: the integer forms, the UTF-8 string, binary data, and
// the bounds check every field goes through.
import { exit } from "runtime:process";
import { ByteReader, ByteWriter } from "../../dist/protocol/wire.js";
import { ok, report, throws } from "./assert.mjs";

// Integers are network byte order.
ok(Array.from(new ByteWriter().u16(0x0102).finish()).join() === "1,2", "u16 is big-endian");
ok(Array.from(new ByteWriter().u32(0x01020304).finish()).join() === "1,2,3,4", "u32 is big-endian");
ok(new ByteReader(new Uint8Array([1, 2])).u16() === 0x0102, "u16 reads big-endian");

// A UTF-8 string is two octets of length then the text.
const text = new ByteWriter().utf8("héllo").finish();
ok(text[0] === 0 && text[1] === 6, "the length prefix is six bytes for héllo");
ok(new ByteReader(text).utf8() === "héllo", "a UTF-8 string round-trips");

// Binary data is the same prefix with no text rules.
const binary = new ByteWriter().binary(new Uint8Array([0xff, 0x00, 0xfe])).finish();
ok(binary[1] === 3, "binary data carries its own length");
const readBinary = new ByteReader(binary).binary();
ok(readBinary.length === 3 && readBinary[0] === 0xff, "binary data round-trips raw bytes");

// The variable byte integer works through the reader and writer too.
const varint = new ByteWriter().varint(300).finish();
ok(new ByteReader(varint).varint() === 300, "a variable byte integer round-trips");

// U+0000 and malformed UTF-8 are both refused, on both sides.
await throws(() => new ByteWriter().utf8("a\u0000b"), "a string with U+0000 cannot be written");
await throws(
  () => new ByteReader(new Uint8Array([0, 1, 0])).utf8(),
  "a string with U+0000 cannot be read",
);
await throws(
  () => new ByteReader(new Uint8Array([0, 2, 0xff, 0xfe])).utf8(),
  "malformed UTF-8 is refused rather than replaced",
);
await throws(
  () => new ByteWriter().utf8("a".repeat(70000)),
  "a string past 65535 bytes cannot be written",
);

// The bounds check: a short buffer is a protocol error, never a read past the end.
await throws(() => new ByteReader(new Uint8Array([1])).u16(), "a short u16 throws");
await throws(() => new ByteReader(new Uint8Array([])).u8(), "reading past the end throws");
await throws(() => new ByteReader(new Uint8Array([0, 5, 1])).utf8(), "a truncated string throws");

// sections and rest.
const reader = new ByteReader(new Uint8Array([1, 2, 3, 4, 5]));
const section = reader.section(3);
ok(section.remaining === 3, "a section is a sub-reader over the next n bytes");
ok(section.rest().length === 3, "rest() gives every remaining byte");
ok(reader.remaining === 2, "the section advanced the outer reader past its bytes");

// The writer grows past its initial capacity.
const grown = new ByteWriter(4);
for (let i = 0; i < 100; i++) grown.u8(i & 0xff);
ok(grown.length === 100, "the writer grows to fit");

// done/offset bookkeeping.
const track = new ByteReader(new Uint8Array([9, 8]));
ok(track.offset === 0 && !track.done, "a fresh reader is at the start");
track.u8();
ok(track.offset === 1 && track.remaining === 1 && !track.done, "offset and remaining track reads");
track.u8();
ok(track.done, "the reader is done after its bytes");

exit(report("wire") > 0 ? 1 : 0);
