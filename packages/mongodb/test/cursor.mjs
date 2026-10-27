// Cursors: batches fetched with getMore, chaining, and releasing a cursor that
// was left early.
import { connect } from "runtime:db";
import { env } from "runtime:process";
import { driver } from "../dist/index.js";
import { is, ok, report } from "./assert.mjs";

await using client = await connect(env.MONGODB_URL, { driver });
const c = client.db().collection("cursor");
await c.drop();
await c.insertMany(Array.from({ length: 250 }, (_, i) => ({ i })));

is((await c.find({}, { batchSize: 7 }).toArray()).length, 250, "toArray reads every batch");
is(
  await c.find({}).sort({ i: -1 }).skip(10).limit(3).project({ _id: 0 }).toArray(),
  [{ i: 239 }, { i: 238 }, { i: 237 }],
  "sort, skip, limit and project chain",
);
let n = 0;
for await (const _ of c.find({ i: { $lt: 20 } }, { batchSize: 3 })) n++;
is(n, 20, "for await reads every batch");

const open = async () => {
  const status = await client.command({ serverStatus: 1 });
  return Number(status.metrics.cursor.open.total);
};
const before = await open();
for await (const doc of c.find({}, { batchSize: 5 })) {
  if (doc.i === 2) break;
}
is(await open(), before, "leaving a for await early releases the cursor on the server");

const cursor = c.find({}, { batchSize: 5 });
await cursor.next();
let changed = null;
try {
  cursor.limit(1);
} catch (e) {
  changed = e.code;
}
is(changed, "ERR_DB_UNSUPPORTED", "a started cursor cannot change");
await cursor.close();
await cursor.close();
is(await cursor.next(), null, "a closed cursor is finished, and closing twice is fine");

const grouped = await c
  .aggregate([
    { $match: { i: { $lt: 10 } } },
    { $group: { _id: { $mod: ["$i", 2] }, n: { $sum: 1 } } },
    { $sort: { _id: 1 } },
  ])
  .toArray();
is(
  grouped,
  [
    { _id: 0, n: 5 },
    { _id: 1, n: 5 },
  ],
  "aggregate",
);
ok(
  Array.isArray(
    await client
      .db("admin")
      .aggregate([{ $currentOp: {} }])
      .toArray(),
  ),
  "a database-level aggregate",
);
if (report("cursor") > 0) throw new Error("cursor failed");
