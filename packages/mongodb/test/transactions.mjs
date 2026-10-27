// Transactions: client.transaction(fn), the session API, retries of transient
// errors and of an unknown commit, and what a transaction refuses. Runs only
// when MONGODB_RS_URL names a replica set with test commands enabled.
import { connect } from "runtime:db";
import { env } from "runtime:process";
import { driver } from "../dist/index.js";
import { is, ok, report } from "./assert.mjs";

if (!env.MONGODB_RS_URL) {
  console.log("skipped: MONGODB_RS_URL is not set (test/replica-set.sh starts one)");
} else {
  const client = await connect(env.MONGODB_RS_URL, { driver });
  const db = client.db();
  const a = db.collection("txn_a");
  const b = db.collection("txn_b");
  for (const c of [a, b]) {
    await c.drop({ writeConcern: { w: "majority" } });
    await db.createCollection(c.collectionName);
  }
  const fail = (commands, data) =>
    client.command({
      configureFailPoint: "failCommand",
      mode: { times: 1 },
      data: { failCommands: commands, ...data },
    });
  const attempt = async (fn) => {
    try {
      return { value: await fn() };
    } catch (error) {
      return { error };
    }
  };

  // client.transaction(fn): both writes, or neither.
  const value = await client.transaction(async (tx) => {
    await tx.db().collection("txn_a").insertOne({ k: 1 });
    await tx.db().collection("txn_b").insertOne({ k: 1 });
    return "done";
  });
  is(value, "done", "transaction(fn) returns what fn returns");
  is(
    [await a.countDocuments({ k: 1 }), await b.countDocuments({ k: 1 })],
    [1, 1],
    "and commits both writes",
  );

  let r = await attempt(() =>
    client.transaction(async (tx) => {
      await tx.db().collection("txn_a").insertOne({ k: 2 });
      throw new Error("changed my mind");
    }),
  );
  is(r.error?.message, "changed my mind", "a throw in fn propagates");
  is(await a.countDocuments({ k: 2 }), 0, "and nothing it wrote is kept");

  // The session API.
  const session = client.startSession();
  session.startTransaction();
  await a.insertOne({ k: 3 }, { session });
  is((await a.findOne({ k: 3 }, { session }))?.k, 3, "a transaction reads its own writes");
  is(await a.findOne({ k: 3 }), null, "which no one else sees before the commit");
  await session.commitTransaction();
  is((await a.findOne({ k: 3 }))?.k, 3, "and everyone sees after it");
  await session.commitTransaction();
  ok(true, "committing again is allowed");
  r = await attempt(() => session.abortTransaction());
  ok(r.error !== undefined, "aborting a committed transaction is refused");

  session.startTransaction();
  await a.insertOne({ k: 4 }, { session });
  await session.abortTransaction();
  is(await a.countDocuments({ k: 4 }), 0, "abortTransaction discards the writes");
  session.startTransaction();
  r = await attempt(() => session.startTransaction());
  ok(r.error !== undefined, "a second transaction cannot start while one is open");
  await a.insertOne({ k: 5 }, { session });
  await session.endSession();
  is(await a.countDocuments({ k: 5 }), 0, "ending a session abandons its open transaction");

  // A transient error retries the whole transaction.
  let calls = 0;
  await fail(["insert"], { errorCode: 112, errorLabels: ["TransientTransactionError"] });
  await client.transaction(async (tx) => {
    calls++;
    await tx.db().collection("txn_a").insertOne({ k: 6 });
  });
  is(calls, 2, "a TransientTransactionError runs the transaction again");
  is(await a.countDocuments({ k: 6 }), 1, "and it commits once");

  // A commit whose connection drops is retried, and applies once.
  calls = 0;
  await fail(["commitTransaction"], { closeConnection: true });
  await client.transaction(async (tx) => {
    calls++;
    await tx.db().collection("txn_a").insertOne({ k: 7 });
  });
  is(
    [calls, await a.countDocuments({ k: 7 })],
    [1, 1],
    "a lost commit is committed again, not rerun",
  );

  // What a transaction refuses.
  r = await attempt(() =>
    client.transaction((tx) =>
      tx
        .db()
        .collection("txn_a")
        .insertOne({ k: 8 }, { writeConcern: { w: 1 } }),
    ),
  );
  ok(/write concern/.test(r.error?.message ?? ""), "an operation's own write concern is refused");
  r = await attempt(() =>
    client.transaction((tx) =>
      tx.db().collection("txn_a").find({}, { readPreference: "secondary" }).toArray(),
    ),
  );
  ok(/primary/.test(r.error?.message ?? ""), "a read preference other than primary is refused");

  await client.command({ configureFailPoint: "failCommand", mode: "off" });
  for (const c of [a, b]) await c.drop();
  await client.close();
}
if (report("transactions") > 0) throw new Error("transactions failed");
