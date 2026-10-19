// URL and server-address parsing: the small public surface that decides where a
// connection goes and what credentials it carries, well before a socket exists.
import { exit } from "runtime:process";
import { NatsError, NatsErrorCode, parseNatsServer, parseNatsUrl } from "../../dist/index.js";
import { is, report, throws } from "./assert.mjs";

// -- scheme, host and port --------------------------------------------------

{
  const plain = parseNatsUrl("nats://broker.example");
  is(plain.hostname, "broker.example", "a plain URL keeps its host");
  is(plain.port, 4222, "and takes the default port");
  is(plain.tls, false, "`nats:` is not TLS");

  const secure = parseNatsUrl("tls://broker.example");
  is(secure.tls, true, "`tls:` is TLS");
  is(secure.port, 4222, "on the same default port");

  is(parseNatsUrl("nats://host:4223").port, 4223, "an explicit port wins");
  is(parseNatsUrl("tls://host:1234").port, 1234, "over the default, either scheme");
  is(
    parseNatsUrl("nats://host:4222/ignored/path").hostname,
    "host",
    "a path after the authority is ignored",
  );
}

// -- credentials: a colon is a user/password, its absence a token -----------

{
  const creds = parseNatsUrl("nats://alice:secret@host:4222");
  is(creds.username, "alice", "a colon splits a user");
  is(creds.password, "secret", "from a password");
  is(creds.token, undefined, "and is not a token");

  const bare = parseNatsUrl("nats://alice@host");
  is(bare.token, "alice", "a userinfo with no colon is a token, not a user");
  is(bare.username, undefined, "with no user");

  const token = parseNatsUrl("nats://tok3n@host");
  is(token.token, "tok3n", "no colon means a token — which is how NATS tells them apart");
  is(token.username, undefined, "with no user");

  const encoded = parseNatsUrl("nats://a%40b:p%3Ass@host");
  is(encoded.username, "a@b", "percent-encoded user parts are decoded");
  is(encoded.password, "p:ss", "and so are passwords");
}

// -- IPv6 -------------------------------------------------------------------

{
  const ipv6 = parseNatsUrl("nats://[::1]:4222");
  is(ipv6.hostname, "::1", "a bracketed IPv6 host is unwrapped");
  is(ipv6.port, 4222, "with its port");
  is(parseNatsUrl("nats://[::1]").port, 4222, "and takes the default port when there is none");
}

// -- refusals ---------------------------------------------------------------

{
  await throws(() => parseNatsUrl("http://host"), "an unknown scheme is refused");
  await throws(() => parseNatsUrl("nats://"), "a URL with no host is refused");
  await throws(() => parseNatsUrl("nats://host:notaport"), "a non-numeric port is refused");
  await throws(() => parseNatsUrl("nats://host:99999"), "an out-of-range port is refused");
  await throws(() => parseNatsUrl("nats://host:0"), "a zero port is refused");
  await throws(() => parseNatsUrl("nonsense"), "a string that is not a URL is refused");
  await throws(() => parseNatsUrl("nats://a%zz@host"), "invalid percent-encoding is refused");

  let code = null;
  try {
    parseNatsUrl("http://host");
  } catch (e) {
    code = e instanceof NatsError ? e.code : null;
  }
  is(code, NatsErrorCode.Protocol, "and raises a Protocol-coded NatsError");
}

// -- parseNatsServer --------------------------------------------------------

{
  is(parseNatsServer("h:4222").port, 4222, "a host:port server keeps its port");
  is(parseNatsServer("h").port, 4222, "a bare host takes the default");
  is(parseNatsServer("h:4222", true).tls, true, "the tls flag is applied to a plain entry");
  is(parseNatsServer("nats://h:4223").tls, false, "a nats:// entry is plain");
  is(parseNatsServer("tls://h").tls, true, "a tls:// entry is TLS");
  is(parseNatsServer("nats://h/ignore").port, 4222, "a full URL entry is parsed as a URL");

  const passed = { hostname: "h", port: 1234, tls: true };
  is(parseNatsServer(passed), passed, "an address object passes through unchanged");

  await throws(() => parseNatsServer(":4222"), "a server with an empty host is refused");
  await throws(() => parseNatsServer("h:x"), "a server with a bad port is refused");
}

exit(report("url") > 0 ? 1 : 0);
