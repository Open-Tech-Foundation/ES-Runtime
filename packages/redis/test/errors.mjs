// Redis's error vocabulary, mapped onto the portable codes.
//
// The rule being checked is the one from the driver-authoring guide: an
// application must be able to branch on what happened without knowing who said
// it, and the backend's own word must survive on `backendCode` so that the
// cases the portable table cannot express are still reachable.
import { exit, env } from "runtime:process";
import { connect, queryAst, DbError, DbErrorCode } from "runtime:db";

import redis from "../dist/index.js";
import { is, ok, report } from "./unit/assert.mjs";

const url = env.REDIS_URL ?? "redis://127.0.0.1:6379";
const db = await connect(url, { driver: redis });
await db.execute(queryAst(["FLUSHDB"]));

/** The error a command threw, or `null`. */
async function failure(command) {
  try {
    await db.execute(queryAst(command));
    return null;
  } catch (e) {
    return e;
  }
}

// -- the shape of a failure -------------------------------------------------

{
  const e = await failure(["NOSUCHCOMMAND"]);
  ok(e !== null, "an unknown command fails");
  ok(e instanceof DbError, `it is a DbError, not a ${e?.constructor?.name}`);
  ok(typeof e.code === "string", "carrying a code");
  is(e.code, DbErrorCode.Syntax, "an unknown command is a syntax error");
  is(e.backendCode, "ERR", "and Redis's own word is kept");
}

is((await failure(["GET"])).code, DbErrorCode.Syntax, "the wrong number of arguments is a syntax error");
is((await failure(["EXPIRE", "k", "soon"])).code, DbErrorCode.Syntax, "so is a value that is not an integer");
is((await failure(["SELECT", "999"])).code, DbErrorCode.Syntax, "and a database index out of range");

// -- the case the portable table deliberately does not cover ----------------

{
  await db.execute(queryAst(["SET", "str", "v"]));
  const e = await failure(["LPUSH", "str", "x"]);
  is(e.code, DbErrorCode.Backend, "WRONGTYPE is not forced into a portable code that would mean something else");
  is(e.backendCode, "WRONGTYPE", "it is reported as itself, which is what an application needs to branch on");
  ok(e.message.includes("wrong kind of value"), "and the server's prose survives");
}

// -- the connection is unharmed ---------------------------------------------

// An error reply is a complete reply. The stream is still aligned, so the next
// command must work — a driver that latched a fatal error here would make every
// typo destroy a connection.
is((await db.execute(queryAst(["SET", "after", "1"]))).changes, 1, "an error reply leaves the connection usable");
{
  const e = await failure(["SET", { not: "a value" }]);
  ok(e !== null, "an argument that cannot be encoded fails");
  is(e.code, DbErrorCode.Unsupported, "as an unsupported parameter rather than a lost connection");
  is((await db.execute(queryAst(["SET", "after2", "1"]))).changes, 1,
    "and the connection survives a bad argument, which was never written to the wire");
}

// -- a connection that really is gone ---------------------------------------

{
  // Once a socket is gone, every later caller gets the same latched error
  // rather than a different symptom of the one dead connection.
  const doomed = await connect(url, { driver: redis });
  await doomed.execute(queryAst(["PING"]));
  const id = (await (await doomed.query(queryAst(["CLIENT", "ID"]))).first()).value;
  // Killed from a *second* connection. Redis defers a self-kill until after it
  // has replied, so asking a connection to kill itself is a race; asking
  // another one is not.
  const executioner = await connect(url, { driver: redis });
  await executioner.execute(queryAst(["CLIENT", "KILL", "ID", String(id)]));
  await executioner.close();

  const first = await (async () => {
    try {
      await doomed.execute(queryAst(["PING"]));
      return null;
    } catch (e) {
      return e;
    }
  })();
  ok(first !== null, "a command on a killed connection fails");
  is(first.code, DbErrorCode.ConnectionLost, "as a lost connection");
  const second = await (async () => {
    try {
      await doomed.execute(queryAst(["PING"]));
      return null;
    } catch (e) {
      return e;
    }
  })();
  is(second.code, DbErrorCode.ConnectionLost, "and so does every one after it");
  ok(doomed.usable === false, "the connection reports itself unusable, which is what a pool asks");
  await doomed.close();
}

await db.execute(queryAst(["FLUSHDB"]));
await db.close();
if (report("errors") > 0) exit(1);
