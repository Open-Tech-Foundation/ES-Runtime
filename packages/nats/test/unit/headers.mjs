// The NATS/1.0 header block: names, repeated names, and the status line that a
// no-responder answer rides in on.
import { exit } from "runtime:process";
import { decodeHeaders, encodeHeaders, ProtocolError } from "../../dist/index.js";
import { is, ok, report, throws } from "./assert.mjs";

const TEXT = new TextDecoder();

// -- an empty block ---------------------------------------------------------

{
  const bytes = encodeHeaders();
  is(TEXT.decode(bytes), "NATS/1.0\r\n\r\n", "an empty block is just the version line");
  const decoded = decodeHeaders(bytes);
  is(Object.keys(decoded.headers).length, 0, "and decodes to no headers");
  is(decoded.status, null, "with no status");
}

// -- values, repeated names, and a status -----------------------------------

{
  const bytes = encodeHeaders(
    { "Nats-Msg-Id": "abc", "X-Multi": ["one", "two"] },
    { code: 503, description: "no responders" },
  );
  is(
    TEXT.decode(bytes),
    "NATS/1.0 503 no responders\r\nNats-Msg-Id: abc\r\nX-Multi: one\r\nX-Multi: two\r\n\r\n",
    "the block encodes status, then names, then values",
  );
  const decoded = decodeHeaders(bytes);
  is(decoded.status?.code, 503, "the status code round-trips");
  is(decoded.status?.description, "no responders", "and the description");
  is(decoded.headers["Nats-Msg-Id"][0], "abc", "a single value round-trips");
  is(decoded.headers["X-Multi"].length, 2, "a repeated name keeps every value");
  is(decoded.headers["X-Multi"][1], "two", "in order");
}

// -- a bare status code -----------------------------------------------------

{
  const decoded = decodeHeaders(encodeHeaders({}, 404));
  is(decoded.status?.code, 404, "a bare status code is written and read");
  is(decoded.status?.description, null, "with no description");
}

// -- refusals ---------------------------------------------------------------

await throws(
  () => decodeHeaders(new TextEncoder().encode("HELLO\r\n\r\n")),
  "a block that does not begin with NATS/1.0 is refused",
);
await throws(
  () => encodeHeaders({ "X-Bad": "line\r\ninjected" }),
  "a header value with CRLF is refused",
);
await throws(() => encodeHeaders({ "X-Bad\r\nName": "v" }), "a header name with CRLF is refused");
await throws(() => encodeHeaders({ "": "v" }), "an empty header name is refused");
await throws(
  () => encodeHeaders({}, { code: 503, description: "ok\r\nX-Injected: yes" }),
  "a status description with CRLF is refused",
);

let protocol = false;
try {
  decodeHeaders(new TextEncoder().encode("HELLO\r\n\r\n"));
} catch (e) {
  protocol = e instanceof ProtocolError;
}
ok(protocol, "a malformed block raises a ProtocolError");

exit(report("headers") > 0 ? 1 : 0);
