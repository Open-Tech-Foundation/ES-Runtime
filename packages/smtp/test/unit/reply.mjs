// Replies: one line and many, enhanced status codes, and what is not SMTP.

import { parseReply, ReplyReader } from "../../dist/protocol/reply.js";
import { is, ok, report, throws } from "./assert.mjs";

// A single line, with and without an enhanced code.
is(parseReply(["250 OK"]), { code: 250, enhanced: null, lines: ["OK"] }, "a plain reply");
is(
  parseReply(["550 5.1.1 <nobody@example.com>: user unknown"]),
  { code: 550, enhanced: "5.1.1", lines: ["<nobody@example.com>: user unknown"] },
  "the enhanced code is lifted out",
);
// A status whose class does not match the reply's is text, not a status.
is(parseReply(["250 5.4 GHz"]).enhanced, null, "digits that are not a status stay text");
is(parseReply(["250"]).lines, [""], "a bare code");

// EHLO's multi-line form.
const ehlo = parseReply([
  "250-mail.example.com",
  "250-PIPELINING",
  "250-SIZE 35882577",
  "250 AUTH PLAIN LOGIN",
]);
is(ehlo.code, 250, "multi-line code");
is(
  ehlo.lines,
  ["mail.example.com", "PIPELINING", "SIZE 35882577", "AUTH PLAIN LOGIN"],
  "every line's text",
);

await throws(() => parseReply(["hello"]), "a line with no code is refused");
await throws(() => parseReply(["250-a", "251 b"]), "a code that changes mid-reply is refused");

// The reader, over a stream that delivers replies in awkward pieces.
function stream(chunks) {
  const encoder = new TextEncoder();
  return new ReadableStream({
    start(controller) {
      for (const chunk of chunks) controller.enqueue(encoder.encode(chunk));
      controller.close();
    },
  });
}

const reader = new ReplyReader(
  stream(["220 mail.exam", "ple.com ESMTP\r\n250-first\r", "\n250 second\r\n", "354 go ahead\n"]),
);
is((await reader.read(1000)).lines, ["mail.example.com ESMTP"], "a reply split across chunks");
is((await reader.read(1000)).lines, ["first", "second"], "a multi-line reply split at the CR");
is((await reader.read(1000)).code, 354, "a bare LF ends a line too");
await throws(() => reader.read(1000), "the end of the stream is a lost connection");

// A server that never answers times out rather than hanging.
const silent = new ReplyReader(new ReadableStream({ start() {} }));
const started = Date.now();
try {
  await silent.read(50);
  ok(false, "a silent server times out");
} catch (e) {
  is(e.code, "ERR_SMTP_TIMEOUT", "the timeout has its code");
  ok(Date.now() - started < 1000, "and arrives on time");
}

// Data the server sent after agreeing to STARTTLS is refused, not carried
// into the encrypted session.
const injected = new ReplyReader(stream(["220 go ahead\r\n250 injected\r\n"]));
await injected.read(1000);
await throws(async () => injected.release(), "plaintext buffered at the upgrade is refused");

if (report("reply") > 0) (await import("runtime:process")).exit(1);
