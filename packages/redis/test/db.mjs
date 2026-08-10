// The `runtime:db` surface: Redis reached through `connect()`, as a backend.
//
// This is the half that matters for the design rather than for the user. Redis
// is the first backend to take `queryAst` — the form the contract has carried
// since its first release for exactly this case — so what is checked here is
// that the seam works in both directions, and that a backend which cannot do
// something says so by name rather than by failing strangely.
import { exit, env } from "runtime:process";
import { connect, queryAst, sql, DbErrorCode } from "runtime:db";

import { driver as redis } from "../dist/index.js";
import { is, ok, report } from "./unit/assert.mjs";

const url = env.REDIS_URL ?? "redis://127.0.0.1:6379";

/** Runs `fn` and returns the `code` of whatever it threw, or `null`. */
async function codeOf(fn) {
  try {
    await fn();
    return null;
  } catch (e) {
    return e.code ?? null;
  }
}

const db = await connect(url, { driver: redis });
await db.execute(queryAst(["FLUSHDB"]));

is(db.backend, "redis", "the scheme found the backend");
is(db.dialect.name, "redis", "and its dialect");

// -- the query form ---------------------------------------------------------

is(db.dialect.supports.queryAst, true, "the backend declares it takes an AST");
is(db.dialect.supports.sqlText, false, "and that it does not take SQL");

is(await codeOf(() => db.query("SELECT 1")), DbErrorCode.QueryForm,
  "SQL text is refused by name rather than sent to a server that would not parse it");
is(await codeOf(() => db.query(sql`SELECT ${1}`)), DbErrorCode.QueryForm,
  "and so is a sql`` template");
is(await codeOf(() => db.query(queryAst("GET k"))), DbErrorCode.QueryForm,
  "an AST that is not a command array is refused too");
is(await codeOf(() => db.query(queryAst([]))), DbErrorCode.QueryForm, "an empty command is refused");

// -- execute ----------------------------------------------------------------

is((await db.execute(queryAst(["SET", "k", "v"]))).changes, 1, "a status reply is one change");
is((await db.execute(queryAst(["DEL", "k"]))).changes, 1, "an integer reply is that integer");
is((await db.execute(queryAst(["DEL", "k"]))).changes, 0, "deleting nothing changes nothing");
is((await db.execute(queryAst(["SET", "k", "v", "NX"]))).changes, 1, "SET NX that applied");
is((await db.execute(queryAst(["SET", "k", "w", "NX"]))).changes, 0, "SET NX that did not");
is((await db.execute(queryAst(["SET", "k", "v"]))).lastInsertRowid, null,
  "Redis has no insert id, and says so rather than inventing one");

// Positional parameters append to the command, which is what makes one command
// reusable across many argument sets.
is((await db.execute(queryAst(["SET"]), ["p", "1"])).changes, 1, "positional arguments are appended");
is(await (await db.query(queryAst(["GET"]), ["p"])).first().then((r) => r.value), "1", "and reach the server");

// -- query, and the row shapes ----------------------------------------------

await db.execute(queryAst(["RPUSH", "list", "a", "b", "c"]));
{
  const rows = await db.query(queryAst(["LRANGE", "list", 0, -1]));
  is(rows.columns.map((c) => c.name).join(","), "value", "an aggregate is one column");
  is((await rows.toArray()).map((r) => r.value), ["a", "b", "c"], "one row per element, in order");
}
{
  const rows = await db.query(queryAst(["GET", "k"]));
  is((await rows.first()).value, "v", "a scalar is one row");
}
{
  is(await (await db.query(queryAst(["GET", "absent"]))).first(), null,
    "a null reply is no rows — the same answer an empty result set gives everywhere");
}
{
  await db.execute(queryAst(["HSET", "h", "one", "1", "two", "2"]));
  const rows = await db.query(queryAst(["HGETALL", "h"]));
  is(rows.columns.map((c) => c.name).join(","), "field,value", "a map is two columns");
  is((await rows.toArray()).map((r) => r.toObject()), [{ field: "one", value: "1" }, { field: "two", value: "2" }],
    "with the pairs as rows");
}
{
  const rows = await db.query(queryAst(["LRANGE", "absent", 0, -1]));
  let seen = 0;
  for await (const _row of rows) seen++;
  is(seen, 0, "an empty result iterates zero times");
}

// A result is complete when it has been read: RESP has no cursor, so the
// connection is free before the caller touches a row. A pool depends on this.
ok((await db.query(queryAst(["LRANGE", "list", 0, -1]))).exhausted, "every result is exhausted");

// Rows are lazy views, and that must hold for this backend as for the built-ins.
{
  const row = await (await db.query(queryAst(["GET", "k"]))).first();
  is(JSON.stringify(row), '{"value":"v"}', "a row serializes as its columns");
  is(Object.keys({ ...row }).length, 0, "spreading it reaches nothing");
  is(Object.getOwnPropertySymbols({ ...row }).length, 0, "and leaks no buffer through a symbol");
}

// A caller who stops early leaves the connection perfectly usable, because
// there was never a cursor to abandon.
{
  await db.execute(queryAst(["DEL", "big"]));
  await db.execute(queryAst(["RPUSH", "big", ...Array.from({ length: 5000 }, (_, i) => String(i))]));
  let seen = 0;
  for await (const _row of await db.query(queryAst(["LRANGE", "big", 0, -1]))) {
    if (++seen === 3) break;
  }
  is(seen, 3, "stopped after three rows");
  is((await (await db.query(queryAst(["LLEN", "big"]))).first()).value, 5000,
    "and the connection answered the next query");
}

// A reply larger than one batch still comes back whole and in order.
{
  const rows = await (await db.query(queryAst(["LRANGE", "big", 0, -1]))).toArray();
  is(rows.length, 5000, "5000 rows across many batches");
  is(rows[0].value, "0", "the first");
  is(rows[4999].value, "4999", "and the last");
}

// -- executeMany ------------------------------------------------------------

{
  await db.execute(queryAst(["FLUSHDB"]));
  const result = await db.executeMany(queryAst(["SET"]), [["a", "1"], ["b", "2"], ["c", "3"]]);
  is(result.changes, 3, "executeMany runs every set");
  is((await (await db.query(queryAst(["MGET", "a", "b", "c"]))).toArray()).map((r) => r.value),
    ["1", "2", "3"], "and all of them landed");
  is((await db.executeMany(queryAst(["SET"]), [])).changes, 0, "an empty batch is a no-op");
}

// -- what this backend cannot do, said by name ------------------------------

is(db.dialect.supports.transactions, false, "Redis declares it has no transactions");
is(await codeOf(() => db.transaction(async () => {})), DbErrorCode.Unsupported,
  "so transaction() refuses rather than sending a BEGIN nothing would understand");
is(db.dialect.supports.savepoints, false, "no savepoints");
is(db.dialect.supports.returning, false, "no RETURNING");
is(db.dialect.supports.namedParameters, false, "and no named parameters");
is(await codeOf(() => db.execute(queryAst(["SET", "k"]), { name: "v" })), DbErrorCode.Unsupported,
  "an object of named parameters is refused");
is(await codeOf(() => db.dialect.placeholder(1)), DbErrorCode.QueryForm,
  "asking a Redis dialect for a placeholder fails loudly instead of answering $1");

// A command that would change what arrives on the socket is refused, because
// this reader expects one reply per command and would otherwise desynchronize.
is(await codeOf(() => db.execute(queryAst(["SUBSCRIBE", "channel"]))), DbErrorCode.Unsupported,
  "SUBSCRIBE is refused by name");
is(await codeOf(() => db.execute(queryAst(["MONITOR"]))), DbErrorCode.Unsupported, "and so is MONITOR");

// A blocking command holds the connection for its timeout, which is the
// caller's to choose — but a timeout of 0 never gives it back at all, and would
// stop every other command on the connection for the life of the process.
is(await codeOf(() => db.query(queryAst(["BLPOP", "q", "0"]))), DbErrorCode.Unsupported,
  "an unbounded BLPOP is refused");
is(await codeOf(() => db.query(queryAst(["XREAD", "BLOCK", "0", "STREAMS", "s", "$"]))),
  DbErrorCode.Unsupported, "and an unbounded XREAD");
{
  // The refusal happens before anything is written, so the connection is
  // untouched by it.
  const e = await (async () => {
    try {
      await db.query(queryAst(["BLPOP", "q", "0"]));
      return null;
    } catch (err) {
      return err;
    }
  })();
  ok(e.message.includes("timeout"), "the message says how to fix it");
  is((await db.execute(queryAst(["SET", "after-refusal", "1"]))).changes, 1,
    "and the connection is unharmed, because nothing reached the wire");
}

// -- signals ----------------------------------------------------------------

{
  const already = AbortSignal.abort(new Error("too late"));
  let message = "ran anyway";
  try {
    await db.query(queryAst(["GET", "a"]), [], { signal: already });
  } catch (e) {
    message = e.message;
  }
  is(message, "too late", "a pre-aborted signal never reaches the server");

  const quiet = new AbortController();
  const rows = await (await db.query(queryAst(["GET", "a"]), [], { signal: quiet.signal })).toArray();
  is(rows.length, 1, "an unaborted signal changes nothing");
  ok((await (await db.query(queryAst(["GET", "a"]))).first()) !== null, "and the connection survived both");
}

// -- closing ----------------------------------------------------------------

await db.close();
await db.close(); // idempotent
is(await codeOf(() => db.query(queryAst(["GET", "a"]))), DbErrorCode.Closed,
  "a closed connection refuses work rather than hanging");

if (report("db") > 0) exit(1);
