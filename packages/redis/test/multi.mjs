// MULTI/EXEC — which is deliberately not `transaction(fn)`.
import { exit, env } from "runtime:process";
import { connect, DbError, DbErrorCode } from "runtime:db";

import { driver as redis } from "../dist/index.js";
import { is, ok, report } from "./unit/assert.mjs";

const url = env.REDIS_URL ?? "redis://127.0.0.1:6379";
const r = await connect(url, { driver: redis });
await r.flushdb();

// -- the happy path ---------------------------------------------------------

{
  const tx = r.multi();
  tx.set("a", "1");
  tx.incr("counter");
  tx.rpush("list", "x", "y");
  is(tx.size, 3, "commands are queued rather than sent");

  const results = await tx.exec();
  is(results, ["OK", 1, 2], "exec answers one result per command, in order");
  is(await r.get("a"), "1", "and they applied");
  is(await r.lrange("list", 0, -1), ["x", "y"], "all of them");
}

{
  // Each queued call is also a promise for its own result, which is the nicer
  // way to read a transaction that mixes commands.
  const tx = r.multi();
  const set = tx.set("b", "2");
  const value = tx.get("b");
  const n = tx.incr("counter");
  await tx.exec();
  is(await set, "OK", "a queued command's promise settles at exec");
  is(await value, "2", "with its own result");
  is(await n, 2, "and the counter carried on from the first transaction");
}

// -- nothing interleaves ----------------------------------------------------

{
  // The property MULTI actually gives: no other client's command lands in the
  // middle. Two concurrent increment-pairs must never leave the two counters
  // disagreeing.
  await r.del("x", "y");
  const other = await connect(url, { driver: redis });
  await Promise.all([
    (async () => {
      for (let i = 0; i < 20; i++) {
        const tx = r.multi();
        tx.incr("x");
        tx.incr("y");
        await tx.exec();
      }
    })(),
    (async () => {
      for (let i = 0; i < 20; i++) {
        const tx = other.multi();
        tx.incr("x");
        tx.incr("y");
        await tx.exec();
      }
    })(),
  ]);
  is(await r.get("x"), "40", "both clients' transactions applied");
  is(await r.get("y"), await r.get("x"), "and the pair never came apart");
  await other.close();
}

// -- the thing it does NOT do -----------------------------------------------

{
  // A command that fails at *exec* time does not roll back the ones beside it.
  // This is the whole reason MULTI is not wired to transaction(fn).
  await r.flushdb();
  await r.set("str", "not-a-list");
  const tx = r.multi();
  tx.set("before", "1");
  tx.call(["LPUSH", "str", "boom"]);   // WRONGTYPE at exec time
  tx.set("after", "1");
  const results = await tx.exec();

  is(results.length, 3, "every command reported");
  ok(results[1] instanceof DbError, "the failing one is a DbError in place, not a throw");
  is(results[1].backendCode, "WRONGTYPE", "carrying Redis's own word");
  is(await r.get("before"), "1", "the command before it applied");
  is(await r.get("after"), "1", "and so did the one after — there is no rollback");
}

{
  // The per-command promise is the other way to read the same thing. It
  // *resolves* with the error rather than rejecting: every helper wraps this in
  // an async method of its own, and `tx.set(k, v)` is written for its effect —
  // rejecting would produce an unhandled rejection per queued command, each
  // pointing at a line that did nothing wrong.
  await r.set("str2", "not-a-list");
  const tx = r.multi();
  const bad = tx.call(["LPUSH", "str2", "boom"]);
  const fine = tx.set("beside", "1");
  await tx.exec();
  ok((await bad) instanceof DbError, "a failed command's promise resolves with its error");
  is((await bad).backendCode, "WRONGTYPE", "carrying Redis's own word");
  is(await fine, "OK", "and the one beside it has its ordinary result");
}

// -- refused at queue time --------------------------------------------------

{
  // The one case Redis *does* throw the lot away: a command it refuses as it is
  // queued makes EXEC fail with EXECABORT, and nothing runs.
  await r.del("untouched");
  const tx = r.multi();
  tx.set("untouched", "1");
  tx.call(["NOSUCHCOMMAND"]);
  let error = null;
  try {
    await tx.exec();
  } catch (e) {
    error = e;
  }
  ok(error !== null, "exec throws when the server discarded the transaction");
  is(error.backendCode, "EXECABORT", "with EXECABORT");
  ok(error.message.includes("unknown command"), "and the queue-time reason attached");
  is(await r.get("untouched"), null, "nothing applied — this is the case that is all-or-nothing");
  is(await r.ping(), "PONG", "and the connection is still usable");
}

// -- WATCH ------------------------------------------------------------------

{
  await r.set("watched", "1");
  await r.watch("watched");
  const tx = r.multi();
  tx.set("watched", "2");
  is(await tx.exec(), ["OK"], "an untouched WATCH lets EXEC through");
  is(await r.get("watched"), "2", "and it applied");
}

{
  // Somebody else changes the key between WATCH and EXEC: EXEC is abandoned.
  await r.set("contended", "1");
  await r.watch("contended");
  const other = await connect(url, { driver: redis });
  await other.set("contended", "changed-by-someone-else");
  await other.close();

  const tx = r.multi();
  tx.set("contended", "mine");
  is(await tx.exec(), null, "exec answers null when a WATCHed key moved");
  is(await r.get("contended"), "changed-by-someone-else", "and nothing was overwritten");
}

{
  // The queued promises reject with the portable code for exactly this: an
  // optimistic-concurrency failure.
  await r.set("c2", "1");
  await r.watch("c2");
  const other = await connect(url, { driver: redis });
  await other.set("c2", "moved");
  await other.close();
  const tx = r.multi();
  const queued = tx.set("c2", "mine");
  await tx.exec();
  const outcome = await queued;
  ok(outcome instanceof DbError, "an aborted transaction settles its queued commands too");
  is(outcome.code, DbErrorCode.SerializationFailure,
    "a WATCH abort is a serialization failure, which is what it is everywhere else");
  await r.unwatch();
}

// -- shape and misuse -------------------------------------------------------

{
  is(await r.multi().exec(), [], "an empty transaction is an empty result");

  const tx = r.multi();
  tx.set("k", "v");
  await tx.exec();
  let threw = false;
  try {
    await tx.exec();
  } catch {
    threw = true;
  }
  ok(threw, "a transaction cannot be executed twice");

  const dropped = r.multi();
  const abandoned = dropped.set("never", "1");
  dropped.discard();
  is(await r.get("never"), null, "discard sends nothing");
  ok((await abandoned) instanceof DbError, "and settles what it queued rather than leaving it pending");
}

{
  // transaction(fn) still refuses — MULTI is not that, and saying so is the
  // point of having both.
  let code = null;
  try {
    await r.transaction(async () => {});
  } catch (e) {
    code = e.code;
  }
  is(code, DbErrorCode.Unsupported, "runtime:db's transaction() is still unsupported");
  is(r.dialect.supports.transactions, false, "and the dialect still says so");
}

// -- through a pool ---------------------------------------------------------

{
  // A pool can run one, because the commands were buffered — there is nothing
  // to hold a connection for until exec.
  const pool = await connect(url, { driver: redis, pool: true });
  const tx = pool.multi();
  tx.set("pooled", "1");
  tx.incr("pooled-n");
  is(await tx.exec(), ["OK", 1], "a pool runs a transaction on one connection");
  is(pool.idle, 1, "and gives the connection straight back");
  await pool.close();
}

await r.flushdb();
await r.close();
if (report("multi") > 0) exit(1);
