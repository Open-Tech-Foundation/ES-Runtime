// The wire codec: what the client writes, what the server says, and the parser
// that keeps a command whole no matter how the socket chopped it up.
import { exit } from "runtime:process";
import {
  DEFAULT_MAX_PAYLOAD,
  drain,
  encodeConnect,
  encodeHpub,
  encodePing,
  encodePong,
  encodePub,
  encodeSub,
  encodeUnsub,
  parseInfo,
} from "../../dist/index.js";
import { is, ok, report, throws } from "./assert.mjs";

const ENC = new TextEncoder();
const DEC = new TextDecoder();
const text = (bytes) => DEC.decode(bytes);
const bytes = (s) => ENC.encode(s);

// -- client commands --------------------------------------------------------

{
  const options = {
    verbose: false,
    pedantic: false,
    tls_required: false,
    lang: "esrun",
    protocol: 1,
    echo: true,
    headers: true,
    no_responders: true,
  };
  const encoded = text(encodeConnect(options));
  ok(encoded.startsWith("CONNECT "), "CONNECT leads with its name");
  ok(encoded.endsWith("\r\n"), "and ends with CRLF");
  is(JSON.parse(encoded.slice("CONNECT ".length).trim()), options, "and carries the JSON");
}

is(
  text(encodePub("a.b", null, bytes("hi"))),
  "PUB a.b 2\r\nhi\r\n",
  "PUB carries subject and length",
);
is(
  text(encodePub("a.b", "_INBOX.x", bytes("hi"))),
  "PUB a.b _INBOX.x 2\r\nhi\r\n",
  "PUB with a reply names it before the length",
);
{
  const headerBytes = bytes("NATS/1.0\r\nX: y\r\n\r\n");
  const encoded = text(encodeHpub("a.b", null, headerBytes, bytes("hi")));
  is(
    encoded,
    `HPUB a.b ${headerBytes.length} ${headerBytes.length + 2}\r\nNATS/1.0\r\nX: y\r\n\r\nhi\r\n`,
    "HPUB carries header and total lengths",
  );
}
{
  const encoded = text(encodeHpub("a.b", "_INBOX.x", bytes("NATS/1.0\r\n\r\n"), bytes("")));
  ok(encoded.startsWith("HPUB a.b _INBOX.x "), "HPUB with a reply names it first");
}
is(text(encodeSub("a.b", undefined, 1)), "SUB a.b 1\r\n", "SUB carries subject and sid");
is(text(encodeSub("a.b", "workers", 1)), "SUB a.b workers 1\r\n", "SUB with a queue names it");
is(text(encodeUnsub(1)), "UNSUB 1\r\n", "UNSUB carries the sid");
is(text(encodeUnsub(1, 5)), "UNSUB 1 5\r\n", "UNSUB with a max carries it");
is(text(encodePing()), "PING\r\n", "PING");
is(text(encodePong()), "PONG\r\n", "PONG");

// -- INFO -------------------------------------------------------------------

{
  const info = parseInfo(
    '{"max_payload":1000,"headers":true,"server_name":"n1","connect_urls":["h:4222"]}',
  );
  is(info.maxPayload, 1000, "max_payload is read");
  is(info.headers, true, "header support is read");
  is(info.serverName, "n1", "the server name is read");
  is(info.connectUrls[0], "h:4222", "cluster peers are read");
}
is(parseInfo("{}").maxPayload, DEFAULT_MAX_PAYLOAD, "a missing max_payload takes the default");
is(parseInfo("{}").authRequired, false, "absence reads as false");
await throws(() => parseInfo("not json"), "non-JSON INFO is refused");
await throws(() => parseInfo("[]"), "a JSON array INFO is refused");

// -- parsing commands -------------------------------------------------------

{
  const { commands, rest } = drain(bytes('INFO {"headers":true}\r\nPING\r\nPONG\r\n'));
  is(commands.length, 3, "three commands are read from one chunk");
  is(commands[0].kind, "info", "INFO first");
  is(commands[1].kind, "ping", "then PING");
  is(commands[2].kind, "pong", "then PONG");
  is(rest.length, 0, "with nothing left over");
}

{
  const { commands } = drain(bytes("MSG a.b 3 5\r\nhello\r\n"));
  const msg = commands[0];
  is(msg.kind, "msg", "a MSG is a message");
  is(msg.subject, "a.b", "with its subject");
  is(msg.sid, 3, "its subscription id");
  is(msg.reply, null, "no reply");
  is(text(msg.payload), "hello", "and its payload");
  is(msg.headers, null, "and no header block");
}

{
  const { commands } = drain(bytes("MSG a.b 3 _INBOX.x 2\r\nhi\r\n"));
  const msg = commands[0];
  is(msg.reply, "_INBOX.x", "a MSG reply is read");
  is(text(msg.payload), "hi", "and its payload");
}

{
  const header = "NATS/1.0 503\r\n";
  const { commands } = drain(
    bytes(`HMSG a.b 4 _INBOX.x ${header.length} ${header.length + 1}\r\n${header}!\r\n`),
  );
  const msg = commands[0];
  is(msg.kind, "msg", "an HMSG is a message");
  is(text(msg.headers), header, "carrying its header bytes");
  is(text(msg.payload), "!", "and its payload");
}

{
  const { commands } = drain(bytes("-ERR 'Authorization Violation'\r\n"));
  is(commands[0].kind, "err", "an -ERR is an error");
  is(commands[0].message, "Authorization Violation", "with its text unquoted");
}

{
  const { commands } = drain(bytes("+OK\r\n"));
  is(commands[0].kind, "ok", "+OK is acknowledged");
}

// -- binary payloads are not text ------------------------------------------

{
  const payload = new Uint8Array([0x00, 0xff, 0x0d, 0x0a, 0x41]);
  const head = bytes(`MSG a.b 1 ${payload.length}\r\n`);
  const buffer = new Uint8Array(head.length + payload.length + 2);
  buffer.set(head, 0);
  buffer.set(payload, head.length);
  buffer.set([0x0d, 0x0a], head.length + payload.length);
  const { commands } = drain(buffer);
  is(commands[0].payload.length, 5, "a binary payload keeps its length");
  is(commands[0].payload[1], 0xff, "and its bytes, CRLF inside included");
  is(commands[0].payload[4], 0x41, "to the end");
}

// -- segmentation -----------------------------------------------------------

{
  const whole = bytes("MSG a.b 3 5\r\nhel");
  const first = drain(whole);
  is(first.commands.length, 0, "a payload still in flight is not a command yet");
  is(first.rest.length, whole.length, "and the whole command stays in the remainder");

  const second = drain(concat(first.rest, bytes("lo\r\nPING\r\n")));
  is(second.commands.length, 2, "the completed chunk yields the message and the ping");
  is(text(second.commands[0].payload), "hello", "with the reassembled payload");
  is(second.commands[1].kind, "ping", "and the command after it");
  is(second.rest.length, 0, "leaving nothing");
}

// -- malformed --------------------------------------------------------------

await throws(() => drain(bytes("MSG a.b 3 notanumber\r\n\r\n")), "a bad byte count is refused");
await throws(() => drain(bytes("MSG a.b\r\n")), "a short MSG line is refused");
await throws(() => drain(bytes("NONSENSE\r\n")), "an unknown line is refused");
await throws(() => drain(bytes("MSG a.b 3 2\r\nabXY")), "a body not closed by CRLF is refused");

exit(report("wire") > 0 ? 1 : 0);

function concat(a, b) {
  const out = new Uint8Array(a.length + b.length);
  out.set(a, 0);
  out.set(b, a.length);
  return out;
}
