// Authentication, and the RESP2 fallback that shares its code path.
//
// These are one test because they are one risk. `HELLO` negotiates the protocol
// *and* authenticates, so a driver that mistakes a failed password for a server
// without RESP3 would quietly connect unauthenticated — and one that mistakes an
// old server for a bad password would make Redis 5 unreachable. The fallback has
// to tell those apart, and this is where that is checked.

import { connect, DbErrorCode, queryAst } from "runtime:db";
import { env, exit, unmask } from "runtime:process";

import { driver as redis } from "../dist/index.js";
import { is, ok, report } from "./unit/assert.mjs";

// `unmask` through, always. A connection string with a password in it is
// exactly what the runtime redacts on the way out of `env`, so reading one
// without unmasking gives the literal string "[redacted]" — which fails as a
// URL rather than as a password, several layers from the cause.
const url = unmask(env.REDIS_AUTH_URL) ?? "redis://:esrun@127.0.0.1:6380";
const plain = unmask(env.REDIS_URL) ?? "redis://127.0.0.1:6379";

// The authenticated server's address with other credentials, for the cases
// that need a wrong password, none, or another user. Built from `url`, so they
// reach the same server the rest of the file does.
function withAuth(username, password) {
  const target = new URL(url);
  target.username = username;
  target.password = password;
  return target.href.replace(/\/$/, "");
}

/** The code a connection attempt failed with, or `null`. */
async function codeOf(target, options = {}) {
  try {
    const db = await connect(target, { driver: redis, ...options });
    await db.close();
    return null;
  } catch (e) {
    return e.code ?? null;
  }
}

// -- a password that works --------------------------------------------------

{
  const r = await connect(url, { driver: redis });
  is(await r.ping(), "PONG", "a password in the URL's userinfo authenticates");
  is(r.protocol, 3, "and RESP3 was still negotiated, in the same round trip");
  await r.close();
}

{
  // The same credential, passed as an option rather than in the URL.
  const r = await connect(withAuth("", ""), { driver: redis, password: "esrun" });
  is(await r.ping(), "PONG", "a password as an option authenticates too");
  await r.close();
}

// -- a password that does not ------------------------------------------------

is(
  await codeOf(withAuth("", "wrong")),
  DbErrorCode.AuthFailed,
  "a wrong password is an authentication failure",
);
is(
  await codeOf(withAuth("", "")),
  DbErrorCode.AuthFailed,
  "and so is no password at all against a server that wants one",
);

// The failure that matters most: a wrong password must not be mistaken for a
// server without RESP3 and quietly downgraded into an unauthenticated session.
{
  let connected = false;
  try {
    const db = await connect(withAuth("", "wrong"), { driver: redis });
    // If this is reached the handshake let a bad credential through.
    await db.execute(queryAst(["PING"]));
    connected = true;
    await db.close();
  } catch {
    /* expected */
  }
  ok(!connected, "a bad password never becomes a protocol downgrade");
}

// -- an ACL username --------------------------------------------------------

{
  const admin = await connect(url, { driver: redis });
  await admin.call(["ACL", "SETUSER", "reader", "on", ">readerpass", "~*", "+@read", "+ping"]);

  const reader = await connect(withAuth("reader", "readerpass"), { driver: redis });
  is(await reader.ping(), "PONG", "an ACL user connects with username and password");
  is(await reader.get("anything"), null, "and may read");
  {
    // A denied capability is the backend's refusal, not the runtime's — it must
    // still arrive as a database error rather than as something stranger.
    let code = null;
    try {
      await reader.set("k", "v");
    } catch (e) {
      code = e.code;
    }
    is(code, DbErrorCode.AuthFailed, "NOPERM on a write maps onto the portable auth code");
  }
  await reader.close();

  await admin.call(["ACL", "DELUSER", "reader"]);
  await admin.close();
}

// -- RESP2 ------------------------------------------------------------------

{
  // Forced, since the containers are modern. It exercises the same fallback a
  // server older than Redis 6 would take, minus the failed HELLO that triggers
  // it — and the point is that the client's answers do not change shape.
  const r = await connect(`${plain}?protocol=2`, { driver: redis });
  is(r.protocol, 2, "?protocol=2 stays on RESP2");
  await r.flushdb();
  is(await r.set("k", "v"), "OK", "SET over RESP2");
  is(await r.get("k"), "v", "GET over RESP2");
  is(await r.get("absent"), null, "RESP2's $-1 is the same null RESP3's _ is");

  // The shapes RESP2 sends differently, which the client is what absorbs.
  await r.hset("h", { a: "1", b: "2" });
  is(
    await r.hgetall("h"),
    { a: "1", b: "2" },
    "HGETALL is an object over RESP2, where the server sent a flat array",
  );
  await r.zadd("z", { one: 1, two: 2 });
  is(
    await r.zrange("z", 0, -1, { withScores: true }),
    [
      ["one", 1],
      ["two", 2],
    ],
    "WITHSCORES pairs correctly over RESP2's interleaved reply",
  );
  is(await r.exists("k"), 1, "an integer reply");
  is(await r.expire("k", 100), true, "1 means yes over RESP2, as true does over RESP3");

  await r.flushdb();
  await r.close();
}

// -- the plain server, which is the default path ----------------------------

{
  // Testing only the configured path is how the PostgreSQL driver shipped a
  // TLS bug that broke every server without it. A password is the configured
  // path here; no password is the default one.
  const r = await connect(plain, { driver: redis });
  is(await r.ping(), "PONG", "a server with no password needs no credentials");
  await r.close();
}

if (report("auth") > 0) exit(1);
