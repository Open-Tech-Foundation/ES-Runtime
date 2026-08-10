// `rediss://` — TLS from the first byte.
//
// Redis has no in-band upgrade: there is no `SSLRequest` to send and no
// plaintext phase to upgrade out of, which makes this the one handshake in the
// package simpler than PostgreSQL's rather than harder. What is worth checking
// is the private authority, because that is what an internal deployment
// actually looks like and what the public roots have never heard of.
//
// Needs a TLS server: eval "$(test/tls-server.sh)" first. Skipped without one,
// because a test that quietly passes when it did not run is worse than no test.
import { exit, env } from "runtime:process";

import { connect } from "runtime:db";

import { driver as redis } from "../dist/index.js";
import { is, ok, report } from "./unit/assert.mjs";

const url = env.REDIS_TLS_URL;
const ca = env.REDIS_CA;

if (!url || !ca) {
  console.log("skip tls — set REDIS_TLS_URL and REDIS_CA (see test/tls-server.sh)");
  exit(0);
}

// -- the certificate has to be trusted --------------------------------------

{
  const r = await connect(url, { driver: redis, tlsCa: ca });
  is(await r.ping(), "PONG", "rediss:// with a private CA connects");
  is(r.protocol, 3, "and negotiates RESP3 over TLS like anywhere else");
  await r.flushdb();
  is(await r.set("k", "v"), "OK", "a command over TLS");
  is(await r.get("k"), "v", "and its reply");
  await r.flushdb();
  await r.close();
}

// -- and refused when it is not ---------------------------------------------

{
  // Without the CA there is nothing to chain to. A driver that connected anyway
  // would be offering encryption without authentication, which is the failure
  // mode TLS exists to prevent.
  let connected = false;
  try {
    const r = await connect(url, { driver: redis });
    await r.ping();
    connected = true;
    await r.close();
  } catch {
    /* expected */
  }
  ok(!connected, "an untrusted certificate is refused rather than accepted quietly");
}

// -- a larger payload, to exercise the record boundaries --------------------

{
  // TLS records are not RESP replies, and a value that spans several of them is
  // where a reader that conflated the two would come apart.
  const r = await connect(url, { driver: redis, tlsCa: ca });
  const big = "x".repeat(300_000);
  await r.set("big", big);
  const read = await r.get("big");
  is(read.length, big.length, "a 300 KB value round-trips across many TLS records");
  ok(read === big, "byte for byte");

  await r.del("big");
  await r.rpush("list", ...Array.from({ length: 2000 }, (_, i) => `item-${i}`));
  const items = await r.lrange("list", 0, -1);
  is(items.length, 2000, "and so does a 2000-element reply");
  is(items[1999], "item-1999", "in order");
  await r.flushdb();
  await r.close();
}

if (report("tls") > 0) exit(1);
