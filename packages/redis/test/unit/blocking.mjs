// Which commands block forever, which is a question about argument positions.
//
// Redis puts the timeout in three different places, and the whole value of the
// check is that it looks in the right one — a table that reads the wrong
// argument would refuse bounded commands and admit unbounded ones, which is
// both halves of wrong at once.
import { exit } from "runtime:process";

import { is, ok, report } from "./assert.mjs";
import { BLOCKING_COMMANDS, blocksForever } from "../../dist/protocol/blocking.js";

const forever = (args) => blocksForever(args) !== null;

// -- the timeout is the last argument ---------------------------------------

ok(forever(["BLPOP", "k", "0"]), "BLPOP with timeout 0 blocks forever");
ok(!forever(["BLPOP", "k", "5"]), "BLPOP with a timeout is bounded");
ok(!forever(["BLPOP", "k", "0.1"]), "a fractional timeout is bounded");
ok(forever(["BLPOP", "a", "b", "c", "0"]), "with many keys, the timeout is still last");
ok(!forever(["BLPOP", "a", "b", "c", "1"]), "and bounded there too");

// The key is called "0" and the timeout is not. Reading the wrong end would
// refuse this.
ok(!forever(["BLPOP", "0", "5"]), "a key that happens to be named 0 is not a timeout");
ok(forever(["BRPOP", "k", 0]), "a numeric 0 counts, not only the string");
ok(forever(["BRPOPLPUSH", "src", "dst", "0"]), "BRPOPLPUSH");
ok(forever(["BLMOVE", "src", "dst", "LEFT", "RIGHT", "0"]), "BLMOVE");
ok(!forever(["BLMOVE", "src", "dst", "LEFT", "RIGHT", "3"]), "BLMOVE, bounded");
ok(forever(["BZPOPMIN", "z", "0"]), "BZPOPMIN");
ok(forever(["BZPOPMAX", "z", "0"]), "BZPOPMAX");

// WAIT and WAITAOF block the same way, in milliseconds rather than seconds —
// which changes nothing about what 0 means.
ok(forever(["WAIT", "1", "0"]), "WAIT with timeout 0");
ok(!forever(["WAIT", "1", "100"]), "WAIT, bounded");
ok(forever(["WAITAOF", "1", "0", "0"]), "WAITAOF reads its last argument, not its middle 0");
ok(!forever(["WAITAOF", "1", "0", "50"]), "WAITAOF, bounded");

// -- the timeout comes first ------------------------------------------------

ok(forever(["BLMPOP", "0", "2", "a", "b", "LEFT"]), "BLMPOP keeps its timeout first");
ok(!forever(["BLMPOP", "1", "2", "a", "b", "LEFT"]), "BLMPOP, bounded");
// The last argument here is LEFT, and numkeys could be 0 in a malformed
// command — neither is the timeout.
ok(!forever(["BLMPOP", "5", "0", "LEFT"]), "BLMPOP does not read numkeys as the timeout");
ok(forever(["BZMPOP", "0", "1", "z", "MIN"]), "BZMPOP");
ok(!forever(["BZMPOP", "2", "1", "z", "MIN"]), "BZMPOP, bounded");

// -- the timeout is behind a keyword ----------------------------------------

ok(forever(["XREAD", "BLOCK", "0", "STREAMS", "s", "$"]), "XREAD BLOCK 0");
ok(!forever(["XREAD", "BLOCK", "1000", "STREAMS", "s", "$"]), "XREAD with a bounded BLOCK");
ok(!forever(["XREAD", "STREAMS", "s", "$"]), "XREAD without BLOCK does not block");
ok(
  forever(["XREAD", "COUNT", "10", "BLOCK", "0", "STREAMS", "s", "$"]),
  "BLOCK is found past other options",
);
ok(
  forever(["XREADGROUP", "GROUP", "g", "c", "BLOCK", "0", "STREAMS", "s", ">"]),
  "XREADGROUP too",
);

// The one that a naive scan gets wrong: a *stream named BLOCK*. Everything
// after STREAMS is keys and IDs, so the scan has to stop there.
ok(
  !forever(["XREAD", "STREAMS", "BLOCK", "0"]),
  "a stream called BLOCK read from id 0 is not a blocking read",
);
ok(
  !forever(["XREAD", "COUNT", "5", "STREAMS", "BLOCK", "0"]),
  "and still not, with an option before it",
);

// -- everything else --------------------------------------------------------

ok(!forever(["GET", "k"]), "an ordinary command does not block");
ok(!forever(["SET", "k", "0"]), "a value of 0 is not a timeout");
ok(!forever(["LPOP", "k", "0"]), "the non-blocking pop is not the blocking one");
ok(!forever([]), "an empty command is somebody else's error");
ok(!forever([Uint8Array.from([1])]), "a command name that is not a string is not matched");

// Case, because a caller may well shout or whisper.
ok(forever(["blpop", "k", "0"]), "lower case is matched");
ok(forever(["xread", "block", "0", "streams", "s", "$"]), "including the keyword");

// -- the name is reported ---------------------------------------------------

is(blocksForever(["BLPOP", "k", "0"]), "BLPOP", "the command's name comes back for the message");
is(blocksForever(["GET", "k"]), null, "and null when there is nothing to say");

ok(BLOCKING_COMMANDS.length >= 11, `${BLOCKING_COMMANDS.length} blocking commands are listed`);

if (report("blocking") > 0) exit(1);
