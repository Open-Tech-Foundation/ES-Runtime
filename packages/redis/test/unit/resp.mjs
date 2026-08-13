// RESP, with no server anywhere near it.
//
// A wire codec does not need a database, and the cases worth pinning are ones a
// live server will not produce on demand: a reply split across three chunks, an
// attribute nobody asked for, RESP2's two spellings of null, a bulk string with
// a CRLF inside it.
import { exit } from "runtime:process";
import { encodeCommand, RespReader } from "../../dist/protocol/resp.js";
import { is, ok, report } from "./assert.mjs";

const encoder = new TextEncoder();

/** A reader over `chunks`, delivered exactly as given — boundaries included. */
function readerOf(...chunks) {
  return new RespReader(
    new ReadableStream({
      start(controller) {
        for (const chunk of chunks) {
          controller.enqueue(typeof chunk === "string" ? encoder.encode(chunk) : chunk);
        }
        controller.close();
      },
    }),
  );
}

// -- writing ----------------------------------------------------------------

is(
  new TextDecoder().decode(encodeCommand(["GET", "key"])),
  "*2\r\n$3\r\nGET\r\n$3\r\nkey\r\n",
  "a command is an array of bulk strings",
);

is(
  new TextDecoder().decode(encodeCommand(["SET", "k", 42])),
  "*3\r\n$3\r\nSET\r\n$1\r\nk\r\n$2\r\n42\r\n",
  "a number argument is stringified the way Redis parses it",
);

is(
  new TextDecoder().decode(encodeCommand(["SET", "k", 9007199254740993n])),
  "*3\r\n$3\r\nSET\r\n$1\r\nk\r\n$16\r\n9007199254740993\r\n",
  "a bigint argument keeps every digit",
);

// Lengths are byte counts, not character counts. Getting this wrong is how a
// driver corrupts every non-ASCII value it writes.
is(
  new TextDecoder().decode(encodeCommand(["SET", "k", "héllo"])),
  "*3\r\n$3\r\nSET\r\n$1\r\nk\r\n$6\r\nhéllo\r\n",
  "a bulk length counts bytes rather than characters",
);

// -- reading, one type at a time --------------------------------------------

{
  const r = readerOf("+OK\r\n:42\r\n$5\r\nhello\r\n_\r\n#t\r\n,3.5\r\n(12345678901234567890\r\n");
  is((await r.next()).value, "OK", "a simple string");
  const integer = await r.next();
  ok(integer.value === 42n, "an integer arrives as a bigint, before anyone narrows it");
  is((await r.next()).value, "hello", "a bulk string");
  is((await r.next()).kind, "null", "RESP3 null");
  is((await r.next()).value, true, "a boolean");
  is((await r.next()).value, 3.5, "a double");
  ok((await r.next()).value === 12345678901234567890n, "a big number past 64 bits");
}

{
  // RESP2 spells null two ways and RESP3 a third. All three mean absence.
  const r = readerOf("$-1\r\n*-1\r\n");
  is((await r.next()).kind, "null", "a bulk string of length -1 is null");
  is((await r.next()).kind, "null", "an array of length -1 is null");
}

{
  const r = readerOf("*3\r\n:1\r\n$3\r\ntwo\r\n*1\r\n+deep\r\n");
  const array = await r.next();
  is(array.kind, "array", "an array");
  is(array.value.length, 3, "with three elements");
  is(array.value[2].value[0].value, "deep", "and arrays nest");
}

{
  const r = readerOf("%2\r\n$1\r\na\r\n:1\r\n$1\r\nb\r\n:2\r\n");
  const map = await r.next();
  is(map.kind, "map", "a map");
  is(map.value.length, 2, "with two pairs");
  is(map.value[0][0].value, "a", "keyed in order");
}

{
  const r = readerOf("~2\r\n$1\r\na\r\n$1\r\nb\r\n");
  is((await r.next()).kind, "set", "a set is distinguishable from an array");
}

{
  const r = readerOf("=15\r\ntxt:hello world\r\n");
  const verbatim = await r.next();
  is(verbatim.value, "hello world", "a verbatim string loses its format tag");
  is(verbatim.verbatim, "txt", "which is kept separately");
}

{
  const r = readerOf("-WRONGTYPE Operation against a key holding the wrong kind of value\r\n");
  const error = await r.next();
  is(error.kind, "error", "an error reply");
  is(error.value.prefix, "WRONGTYPE", "with its prefix split off");
}

{
  // A bare error with no message at all is legal, and must not produce an
  // empty prefix — the prefix is the only thing worth classifying on.
  const r = readerOf("-ERR\r\n");
  is((await r.next()).value.prefix, "ERR", "an error with no message is all prefix");
}

{
  // Attributes are metadata attached to a reply. A client that let them through
  // would hand callers a value whose shape depended on server configuration.
  const r = readerOf("|1\r\n$3\r\nkey\r\n$5\r\nvalue\r\n+OK\r\n");
  is((await r.next()).value, "OK", "an attribute is read and discarded");
}

// -- reading, against hostile framing ---------------------------------------

{
  // The property that matters most: the reader owns no assumption about where a
  // chunk ends. A reply split mid-length, mid-payload and mid-terminator must
  // read exactly the same as one that arrived whole.
  const r = readerOf("$", "11\r\nhel", "lo wo", "rld\r", "\n+next\r\n");
  is((await r.next()).value, "hello world", "a bulk string split across five chunks");
  is((await r.next()).value, "next", "and the reply after it");
}

{
  const r = readerOf("*2\r\n", "$1\r\n", "a\r\n", "$1\r\n", "b\r\n");
  const array = await r.next();
  is(array.value.map((x) => x.value).join(""), "ab", "an array split at every boundary");
}

{
  // Bulk strings are binary-safe: the length says how many bytes to take, so a
  // CRLF inside one is data. A reader that scanned for the terminator instead
  // of counting would truncate here.
  const r = readerOf("$7\r\na\r\nb\r\nc\r\n");
  is((await r.next()).value, "a\r\nb\r\nc", "a CRLF inside a bulk string is data");
}

{
  const r = readerOf("$0\r\n\r\n");
  is((await r.next()).value, "", "an empty bulk string is a value, not a null");
}

{
  const r = readerOf("*0\r\n");
  is((await r.next()).value.length, 0, "an empty array is a value, not a null");
}

{
  // Bytes that are not RESP mean the reader has lost its place, and saying so
  // is the only safe answer — guessing would return whatever the misalignment
  // happened to spell.
  const r = readerOf("garbage\r\n");
  let threw = false;
  try {
    await r.next();
  } catch {
    threw = true;
  }
  ok(threw, "a byte that is not a RESP type is refused");
}

{
  // The peer hanging up mid-reply is not a short read to be retried.
  const r = readerOf("$100\r\ntoo short");
  let threw = false;
  try {
    await r.next();
  } catch (e) {
    threw = e.name === "RespEof";
  }
  ok(threw, "a truncated reply reports the connection closed");
}

if (report("resp") > 0) exit(1);
