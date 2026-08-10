// Hash slots, against published vectors rather than against ourselves.
//
// Every Redis client has to agree with the server byte for byte here, so a
// value this file gets wrong is a key that quietly lives on the wrong node.
import { exit } from "runtime:process";

import { is, ok, report } from "./assert.mjs";
import { SLOTS, crc16, hashSlot, hashTag } from "../../dist/protocol/slots.js";

const bytes = (text) => new TextEncoder().encode(text);

// The CRC16/XMODEM check value, which is the standard's own vector.
is(crc16(bytes("123456789")), 0x31c3, "CRC16/XMODEM of '123456789' is 0x31C3");
is(crc16(bytes("")), 0, "an empty input is 0");

// Slots quoted in Redis's own cluster documentation.
is(hashSlot("foo"), 12182, "foo → 12182");
is(hashSlot("bar"), 5061, "bar → 5061");
is(hashSlot("hello"), 866, "hello → 866");
is(SLOTS, 16384, "a cluster has 16384 slots");

// -- hash tags --------------------------------------------------------------

is(hashTag("foo"), "foo", "a key with no braces hashes whole");
is(hashTag("{user1000}.following"), "user1000", "a tag is what is between the braces");
is(hashTag("foo{bar}"), "bar", "wherever it appears");

// The two that follow from "first" being meant literally, and that clients get
// wrong: an empty tag does not send it looking for a later pair, and the first
// closing brace closes the first opening one whatever is in between.
is(hashTag("foo{}{bar}"), "foo{}{bar}", "an empty tag means the whole key is hashed");
is(hashTag("foo{{bar}}"), "{bar", "the first } closes the first {");
is(hashTag("foo{bar}{zap}"), "bar", "and only the first pair counts");
is(hashTag("{}"), "{}", "a key that is only an empty tag hashes whole");
is(hashTag("}{"), "}{", "a closing brace before an opening one is not a tag");
is(hashTag("{unclosed"), "{unclosed", "an unclosed brace is not a tag");

// The property the tag exists for.
is(
  hashSlot("{user1000}.following"),
  hashSlot("{user1000}.followers"),
  "two keys sharing a tag share a slot — which is what makes multi-key commands possible",
);
is(hashSlot("{user1000}.x"), hashSlot("user1000"), "and it matches the untagged key itself");
ok(hashSlot("foo") !== hashSlot("bar"), "different keys generally differ");

// Binary-safe: a key is bytes, and a non-ASCII one must not go through a
// lossy conversion on its way to a slot.
ok(Number.isInteger(hashSlot("héllo")), "a non-ASCII key hashes to an integer");
ok(hashSlot("héllo") >= 0 && hashSlot("héllo") < SLOTS, "inside the slot range");

// Every slot is in range, checked over a spread of keys rather than asserted.
{
  let lowest = SLOTS;
  let highest = -1;
  for (let i = 0; i < 5000; i++) {
    const slot = hashSlot(`key:${i}`);
    ok(slot >= 0 && slot < SLOTS, `slot ${slot} is in range`);
    if (slot < lowest) lowest = slot;
    if (slot > highest) highest = slot;
  }
  ok(lowest < 500 && highest > SLOTS - 500, `5000 keys spread across the range (${lowest}–${highest})`);
}

if (report("slots") > 0) exit(1);
