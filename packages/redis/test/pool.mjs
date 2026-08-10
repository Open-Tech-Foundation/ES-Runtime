// The pool, and the one decision a protocol-blind pool cannot make for itself.
import { exit, env } from "runtime:process";
import { connect, queryAst, DbErrorCode } from "runtime:db";

import { driver as redis } from "../dist/index.js";
import { is, ok, report } from "./unit/assert.mjs";

const url = env.REDIS_URL ?? "redis://127.0.0.1:6379";

// -- nothing is opened until something is asked -----------------------------

{
  const pool = await connect(url, { driver: redis, pool: true });
  is(pool.size, 0, "a pool costs nothing until it is used");
  is(await pool.ping(), "PONG", "and opens a connection when it is");
  is(pool.size, 1, "one connection");
  is(pool.idle, 1, "returned to the pool");

  // A borrowed-and-returned connection is reused rather than reopened, which is
  // the whole point.
  for (let i = 0; i < 20; i++) await pool.set(`k${i}`, String(i));
  is(pool.size, 1, "twenty commands still used one connection");
  is(await pool.get("k7"), "7", "and the values are there");
  await pool.close();
}

// -- concurrency ------------------------------------------------------------

{
  const pool = await connect(url, { driver: redis, pool: { max: 4 } });
  const results = await Promise.all(
    Array.from({ length: 50 }, (_, i) => pool.set(`c${i}`, String(i))),
  );
  is(results.length, 50, "fifty concurrent commands all completed");
  ok(pool.size <= 4, `and the pool never exceeded its maximum (${pool.size})`);

  const read = await Promise.all(Array.from({ length: 50 }, (_, i) => pool.get(`c${i}`)));
  is(read.join(","), Array.from({ length: 50 }, (_, i) => String(i)).join(","),
    "every reply went to the caller that asked for it");
  await pool.close();
}

// -- release(clean), which needs the protocol -------------------------------

{
  const pool = await connect(url, { driver: redis, pool: true });
  await pool.ping();
  is(pool.idle, 1, "a connection is idle after an ordinary command");

  // `SELECT` moves the connection to another database. Handing that to the next
  // borrower would silently point their keys at a different dataset, so the
  // driver declares it unclean and the pool throws it away.
  await pool.call(["SELECT", "3"]);
  is(pool.size, 0, "a connection left on another database is destroyed rather than reused");

  await pool.ping();
  is(pool.size, 1, "and the pool opens a fresh one for the next caller");
  await pool.close();
}

{
  // The same rule for an unfinished MULTI: a connection holding a queue nobody
  // is going to EXEC is not fit for anyone else.
  const pool = await connect(url, { driver: redis, pool: true });
  await pool.call(["MULTI"]);
  is(pool.size, 0, "a connection inside an open MULTI is destroyed on release");
  await pool.close();
}

// -- a connection that died while nobody held it ----------------------------

{
  const pool = await connect(url, { driver: redis, pool: true });
  await pool.ping();
  is(pool.idle, 1, "one idle connection");

  // Killed from outside, exactly as a server restart would. The pool's
  // `validate` is what notices, on the way out rather than at the caller.
  const executioner = await connect(url, { driver: redis });
  await executioner.execute(queryAst(["CLIENT", "KILL", "TYPE", "normal", "LADDR", "*"]));
  await executioner.close().catch(() => {});
  // Give the close a moment to reach us.
  await new Promise((resolve) => setTimeout(resolve, 100));

  is(await pool.ping(), "PONG", "the pool replaced a connection that died while idle");
  await pool.close();
}

// -- withConnection ---------------------------------------------------------

{
  const pool = await connect(url, { driver: redis, pool: true });
  // The escape hatch for the few things that are stateful across commands.
  const answer = await pool.withConnection(async (connection) => {
    await connection.call(["SET", "held", "1"]);
    return connection.call(["GET", "held"]);
  });
  is(answer, "1", "withConnection holds one connection for the whole of it");
  await pool.close();
}

// -- rows through a pool ----------------------------------------------------

{
  const pool = await connect(url, { driver: redis, pool: true });
  await pool.del("plist");
  await pool.rpush("plist", "a", "b", "c");
  const rows = await pool.query(["LRANGE", "plist", 0, -1]);
  // The connection went back before the caller read a row, and that is correct
  // here: a RESP reply is complete once read, so there is no cursor holding it.
  ok(rows.exhausted, "a pooled result is exhausted");
  is(pool.idle, 1, "so the connection was returned before the rows were read");
  is((await rows.toArray()).map((r) => r.value), ["a", "b", "c"], "and the rows are still readable");
  await pool.close();
}

// -- a closed pool ----------------------------------------------------------

{
  const pool = await connect(url, { driver: redis, pool: true });
  await pool.ping();
  await pool.close();
  let code = null;
  try {
    await pool.ping();
  } catch (e) {
    code = e.code;
  }
  is(code, DbErrorCode.Closed, "a closed pool refuses new work");
}

// -- through runtime:db's registry ------------------------------------------

{
  const pool = await connect(url, { driver: redis, pool: { max: 2 } });
  is(await pool.ping(), "PONG", "connect(url, { pool: true }) gives a pool");
  await pool.close();
}

{
  const pool = await connect(url, { driver: redis, pool: true });
  await pool.call(["FLUSHDB"]);
  await pool.close();
}

if (report("pool") > 0) exit(1);
