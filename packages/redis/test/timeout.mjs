// A per-command deadline, and the connection it has to destroy.
import { exit, env } from "runtime:process";
import { connect, DbErrorCode } from "runtime:db";

import { driver as redis } from "../dist/index.js";
import { is, ok, report } from "./unit/assert.mjs";

const url = env.REDIS_URL ?? "redis://127.0.0.1:6379";

{
  const r = await connect(url, { driver: redis, commandTimeout: 300 });
  is(await r.ping(), "PONG", "an ordinary command is well inside the deadline");
  await r.set("k", "v");
  is(await r.get("k"), "v", "and so is another");
  await r.close();
}

{
  // A bounded blocking command that outlasts the deadline is the honest way to
  // make a reply late without breaking the server.
  const r = await connect(url, { driver: redis, commandTimeout: 200 });
  const started = Date.now();
  let code = null;
  try {
    await r.call(["BLPOP", "nothing-ever", "3"]);
  } catch (e) {
    code = e.code;
  }
  is(code, DbErrorCode.Timeout, "a command that outruns the deadline times out");
  ok(Date.now() - started < 2000, "promptly, rather than waiting for the command");

  // The reply is still coming, so the connection cannot be reused — reading it
  // later would answer the *next* command with this one's result.
  ok(!r.usable, "and the connection is destroyed, because its reply is still in flight");
  let after = null;
  try {
    await r.ping();
  } catch (e) {
    after = e.code;
  }
  is(after, DbErrorCode.ConnectionLost, "so a later command on it fails rather than desyncing");
  await r.close();
}

{
  // With reconnect on, that cost is one dropped socket.
  const r = await connect(url, { driver: redis, commandTimeout: 200, reconnect: true });
  try {
    await r.call(["BLPOP", "nothing-ever", "3"]);
  } catch {
    /* expected */
  }
  is(await r.ping(), "PONG", "reconnect turns a timed-out connection into a fresh one");
  is(await r.get("k"), "v", "which is the same server");
  await r.close();
}

{
  // Settable from the connection string too.
  const r = await connect(`${url}?command_timeout=200`, { driver: redis });
  let code = null;
  try {
    await r.call(["BLPOP", "nothing-ever", "3"]);
  } catch (e) {
    code = e.code;
  }
  is(code, DbErrorCode.Timeout, "?command_timeout applies");
  await r.close();
}

{
  const r = await connect(url, { driver: redis });
  const started = Date.now();
  is(await r.call(["BLPOP", "nothing-ever", "1"]), null, "with no deadline a command runs to completion");
  ok(Date.now() - started >= 900, "however long it takes");
  await r.close();
}

// -- binary from the URL ----------------------------------------------------

{
  const r = await connect(`${url}?binary=1`, { driver: redis });
  await r.set("bytes", new Uint8Array([0xff, 0x00, 0xfe]));
  const value = await r.get("bytes");
  ok(value instanceof Uint8Array, "?binary=1 hands values back as bytes");
  is([...value], [255, 0, 254], "which survived exactly");
  await r.del("bytes");
  await r.close();
}

if (report("timeout") > 0) exit(1);
