// Retryable writes and reads, driven by the server's failCommand fail point.
// Runs only when MONGODB_RS_URL names a replica set with test commands enabled
// (test/replica-set.sh): a standalone takes no retryable writes.
import { connect } from "runtime:db";
import { env } from "runtime:process";
import { driver } from "../dist/index.js";
import { is, ok, report } from "./assert.mjs";

if (!env.MONGODB_RS_URL) {
  console.log("skipped: MONGODB_RS_URL is not set (test/replica-set.sh starts one)");
} else {
  const client = await connect(env.MONGODB_RS_URL, { driver });
  const c = client.db().collection("retry");
  await c.drop();
  // Fail the next `times` of these commands on the primary, as described.
  const fail = (commands, data, times = 1) =>
    client.command({
      configureFailPoint: "failCommand",
      mode: { times },
      data: { failCommands: commands, ...data },
    });
  const off = () => client.command({ configureFailPoint: "failCommand", mode: "off" });
  const attempt = async (fn) => {
    try {
      return { value: await fn() };
    } catch (error) {
      return { error };
    }
  };

  await fail(["insert"], { closeConnection: true });
  let r = await attempt(() => c.insertOne({ k: "net" }));
  ok(r.error === undefined, "an insert whose connection drops is retried");
  is(await c.countDocuments({ k: "net" }), 1, "and lands once");

  await fail(["insert"], { errorCode: 189 });
  r = await attempt(() => c.insertOne({ k: "stepdown" }));
  ok(r.error === undefined, "an insert failing with a retryable error code is retried");
  is(await c.countDocuments({ k: "stepdown" }), 1, "and lands once");

  await fail(["insert"], {
    writeConcernError: { code: 91, errmsg: "shutting down", errorLabels: ["RetryableWriteError"] },
  });
  r = await attempt(() => c.insertOne({ k: "wce" }));
  ok(r.error === undefined, "a retryable write concern error is retried");
  is(await c.countDocuments({ k: "wce" }), 1, "and the document is written once");

  await fail(["findAndModify"], { closeConnection: true });
  r = await attempt(() =>
    c.findOneAndUpdate({ k: "net" }, { $inc: { n: 1 } }, { returnDocument: "after" }),
  );
  is(r.value?.n, 1, "findOneAndUpdate is retried, and applies once");

  await fail(["update"], { closeConnection: true });
  r = await attempt(() => c.updateMany({}, { $set: { all: true } }));
  is(r.error?.code, "ERR_DB_CONNECTION_LOST", "updateMany is not retried");
  await fail(["update"], { closeConnection: true });
  r = await attempt(() => c.updateOne({ k: "net" }, { $set: { one: true } }));
  ok(r.error === undefined, "updateOne is");

  const strict = await connect(env.MONGODB_RS_URL, {
    driver,
    retryWrites: false,
    retryReads: false,
  });
  const sc = strict.db().collection("retry");
  await fail(["insert"], { closeConnection: true });
  r = await attempt(() => sc.insertOne({ k: "no-retry" }));
  is(r.error?.code, "ERR_DB_CONNECTION_LOST", "retryWrites=false does not retry");
  await fail(["find"], { closeConnection: true });
  r = await attempt(() => sc.findOne({}));
  is(r.error?.code, "ERR_DB_CONNECTION_LOST", "retryReads=false does not retry");
  await strict.close();

  // Reads.
  await fail(["find"], { closeConnection: true });
  r = await attempt(() => c.find({ k: "net" }).toArray());
  is(r.value?.length, 1, "a find whose connection drops is retried");
  await fail(["distinct"], { errorCode: 11600 });
  r = await attempt(() => c.distinct("k"));
  ok(Array.isArray(r.value), "distinct is retried");
  await fail(["aggregate"], { errorCode: 91 });
  r = await attempt(() => c.countDocuments({}));
  ok(r.value > 0, "countDocuments is retried");
  await fail(["count"], { closeConnection: true });
  r = await attempt(() => c.estimatedDocumentCount());
  ok(r.value > 0, "estimatedDocumentCount is retried");

  await c.insertMany(Array.from({ length: 10 }, (_, i) => ({ g: i })));
  const cursor = c.find({ g: { $gte: 0 } }, { batchSize: 2 });
  await cursor.next();
  await fail(["getMore"], { closeConnection: true });
  r = await attempt(() => cursor.toArray());
  is(r.error?.code, "ERR_DB_CONNECTION_LOST", "a getMore is not retried");

  await fail(["ping"], { closeConnection: true });
  r = await attempt(() => client.command({ ping: 1 }));
  is(r.error?.code, "ERR_DB_CONNECTION_LOST", "nor is a command run with command()");

  await off();
  await c.drop();
  await client.close();
}
if (report("retry") > 0) throw new Error("retry failed");
