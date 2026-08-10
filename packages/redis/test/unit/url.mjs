// Connection strings, which are the part users get wrong and the part a driver
// can check without a server.
import { exit } from "runtime:process";
import { is, ok, report } from "./assert.mjs";
import { parseConnectionString } from "../../dist/url.js";

is(parseConnectionString("redis://localhost").port, 6379, "the default port");
is(parseConnectionString("redis://localhost").host, "localhost", "the host");
is(parseConnectionString("redis://").host, "localhost", "an empty host falls back to localhost");
ok(parseConnectionString("redis://localhost").tls !== true, "redis:// is plaintext");
ok(parseConnectionString("rediss://localhost").tls === true, "rediss:// is TLS from the first byte");

is(parseConnectionString("redis://host:6380").port, 6380, "an explicit port");

// The path is a database *index*. Redis databases are numbered, and reading the
// path as a name is the mistake a driver written against PostgreSQL makes.
is(parseConnectionString("redis://host/3").db, 3, "the path is a database index");
is(parseConnectionString("redis://host/0").db, 0, "including zero");
is(parseConnectionString("redis://host").db, undefined, "and is optional");
is(parseConnectionString("redis://host/?db=4").db, 4, "?db= is the other spelling");
is(parseConnectionString("redis://host/2?db=4").db, 2, "the path wins, being the registered form");

{
  let threw = false;
  try {
    parseConnectionString("redis://host/mydb");
  } catch {
    threw = true;
  }
  ok(threw, "a database *name* is refused rather than silently ignored");
}

// The pre-ACL spelling: an empty username with a password means `default`.
is(parseConnectionString("redis://:secret@host").password, "secret", "a password with no username");
is(parseConnectionString("redis://:secret@host").username, undefined, "leaves the user unset");
is(parseConnectionString("redis://alice:secret@host").username, "alice", "an ACL username");

// Percent-encoding, because a password that needs it is exactly the password
// someone will have.
is(
  parseConnectionString("redis://:p%40ss%3Aword@host").password,
  "p@ss:word",
  "a percent-encoded password is decoded",
);

// Seconds in the URL, milliseconds in the options object.
is(parseConnectionString("redis://host?connect_timeout=5").connectTimeout, 5000, "connect_timeout is seconds");

is(parseConnectionString("redis://host?binary=1").binary, true, "?binary=1 hands back bytes");
is(parseConnectionString("redis://host?binary=true").binary, true, "and ?binary=true");
is(parseConnectionString("redis://host?binary=0").binary, false, "?binary=0 turns it off explicitly");
is(parseConnectionString("redis://host").binary, undefined, "and it is unset by default");
is(parseConnectionString("redis://host?command_timeout=250").commandTimeout, 250,
  "?command_timeout is milliseconds, unlike connect_timeout's seconds");

is(parseConnectionString("redis://host?protocol=2").resp3, false, "?protocol=2 forces RESP2");
is(parseConnectionString("redis://host?protocol=3").resp3, true, "?protocol=3 asks for RESP3");
is(parseConnectionString("redis://host?client_name=web").clientName, "web", "?client_name");

{
  // One place for a credential. A second one is a credential a URL-redacting
  // logger does not know to strip.
  let threw = false;
  try {
    parseConnectionString("redis://host?password=secret");
  } catch {
    threw = true;
  }
  ok(threw, "a password in a query parameter is refused");
}

{
  let threw = false;
  try {
    parseConnectionString("postgres://host");
  } catch {
    threw = true;
  }
  ok(threw, "another scheme is refused by name");
}

// Explicit options beat the URL, which is what makes them useful.
is(parseConnectionString("redis://host:6379", { port: 7000 }).port, 7000, "options override the URL");
is(parseConnectionString("redis://host", { password: "x" }).password, "x", "including credentials");

if (report("url") > 0) exit(1);
