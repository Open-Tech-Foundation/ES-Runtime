// DATA framing: line endings, dot-stuffing, the terminator, the line limit.

import { frame, MAX_LINE_OCTETS } from "../../dist/protocol/data.js";
import { is, ok, report, throws } from "./assert.mjs";

const text = (framed) => new TextDecoder().decode(framed.bytes);

is(
  text(frame("Hello\r\nWorld\r\n")),
  "Hello\r\nWorld\r\n.\r\n",
  "a CRLF message gets the terminator",
);
is(
  text(frame("Hello\nWorld")),
  "Hello\r\nWorld\r\n.\r\n",
  "bare LF becomes CRLF, and the last line is ended",
);
is(text(frame("a\rb")), "a\r\nb\r\n.\r\n", "a bare CR is a line break too");
is(text(frame("")), ".\r\n", "an empty message is just the terminator");

// The line that would end the data early, and its lookalikes.
is(text(frame(".\r\n")), "..\r\n.\r\n", "a lone dot is stuffed");
is(
  text(frame("a\r\n.hidden\r\n..two\r\n")),
  "a\r\n..hidden\r\n...two\r\n.\r\n",
  "every leading dot gains one",
);
is(text(frame("a.b\r\n")), "a.b\r\n.\r\n", "a dot mid-line is left alone");

// Size is the message's, not the stuffed bytes'.
is(frame(".\r\n").size, 3, "size counts the message, not the stuffing");
is(frame("ab\n").size, 4, "size counts CRLF after normalising");

ok(!frame("plain ascii").eightBit, "ASCII is 7-bit");
ok(frame("naïve").eightBit, "UTF-8 text is 8-bit");
ok(frame(new Uint8Array([0x41, 0xff])).eightBit, "bytes above 127 are 8-bit");

is(
  frame("x".repeat(MAX_LINE_OCTETS)).size,
  MAX_LINE_OCTETS + 2,
  "a line of exactly 998 octets is allowed",
);
await throws(() => frame("x".repeat(MAX_LINE_OCTETS + 1)), "a longer line is refused");
try {
  frame(`ok\r\n${"y".repeat(1000)}`);
} catch (e) {
  ok(e.message.includes("line 2"), "the error names the line");
  is(e.code, "ERR_SMTP_INVALID_MESSAGE", "and has its code");
}

if (report("data") > 0) (await import("runtime:process")).exit(1);
