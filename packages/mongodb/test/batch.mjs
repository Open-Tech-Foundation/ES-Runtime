// Writes larger than one message are split into batches the server accepts;
// a document larger than the server allows is refused before it is sent.
import { connect } from "runtime:db";
import { env } from "runtime:process";
import { driver } from "../dist/index.js";
import { is, report } from "./assert.mjs";

await using client = await connect(env.MONGODB_URL, { driver });
const c = client.db().collection("batch");
await c.drop();

// 60 documents of 1 MB: past the 48 MB message limit, so at least two messages.
const big = "x".repeat(1024 * 1024);
const docs = Array.from({ length: 60 }, (_, i) => ({ i, big }));
is((await c.insertMany(docs)).insertedCount, 60, "an insert larger than a message is split");
is(await c.countDocuments(), 60, "and every document arrived");

// More statements than one batch may carry.
const small = Array.from({ length: 100_050 }, (_, i) => ({ i }));
await c.drop();
is((await c.insertMany(small)).insertedCount, 100_050, "an insert past maxWriteBatchSize is split");

let tooBig = null;
try {
  await c.insertOne({ huge: "x".repeat(17 * 1024 * 1024) });
} catch (e) {
  tooBig = e.backendCode;
}
is(tooBig, 10334, "a document over 16 MiB is refused as BSONObjectTooLarge");
await c.drop();
if (report("batch") > 0) throw new Error("batch failed");
