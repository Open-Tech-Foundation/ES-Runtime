// The collection API's writes and reads, and what each result reports.
import { connect } from "runtime:db";
import { env } from "runtime:process";
import { driver, ObjectId } from "../dist/index.js";
import { is, ok, report, throws } from "./assert.mjs";

await using client = await connect(env.MONGODB_URL, { driver });
const c = client.db().collection("crud");
await c.drop();

const doc = { name: "Ada" };
const one = await c.insertOne(doc);
ok(one.insertedId instanceof ObjectId, "insertOne gives an ObjectId when the document has none");
is(Object.keys(doc), ["name"], "and leaves the caller's document unchanged");
is((await c.insertOne({ _id: 7, name: "Seven" })).insertedId, 7, "a given _id is kept");

const many = await c.insertMany([
  { name: "Bo", age: 20 },
  { name: "Cy", age: 30 },
  { name: "Di", age: 40 },
]);
is(many.insertedCount, 3, "insertMany counts");
is(Object.keys(many.insertedIds), ["0", "1", "2"], "insertedIds are keyed by position");

is(
  await c.updateOne({ name: "Bo" }, { $inc: { age: 1 } }),
  { acknowledged: true, matchedCount: 1, modifiedCount: 1, upsertedCount: 0, upsertedId: null },
  "updateOne reports what matched and changed",
);
is(
  (await c.updateMany({ age: { $gte: 30 } }, { $set: { senior: true } })).modifiedCount,
  2,
  "updateMany",
);
const up = await c.updateOne({ name: "Ed" }, { $set: { age: 50 } }, { upsert: true });
is([up.matchedCount, up.upsertedCount], [0, 1], "an upsert inserts");
ok(up.upsertedId instanceof ObjectId, "and reports its _id");
is((await c.replaceOne({ name: "Ed" }, { name: "Ed", age: 51 })).modifiedCount, 1, "replaceOne");
await throws(() => c.updateOne({}, { age: 1 }), "an update without operators is refused");
await throws(() => c.replaceOne({}, { $set: { a: 1 } }), "a replacement with operators is refused");

is((await c.findOne({ name: "Bo" })).age, 21, "findOne");
is(await c.findOne({ name: "nobody" }), null, "findOne finds nothing as null");
is(await c.countDocuments({ age: { $gte: 30 } }), 3, "countDocuments");
is(await c.countDocuments({}, { skip: 1, limit: 2 }), 2, "countDocuments with skip and limit");
is(await c.estimatedDocumentCount(), 6, "estimatedDocumentCount");
is((await c.distinct("age", { age: { $lt: 40 } })).sort(), [21, 30], "distinct");

const after = await c.findOneAndUpdate(
  { name: "Cy" },
  { $set: { age: 31 } },
  { returnDocument: "after", projection: { _id: 0, age: 1 } },
);
is(after, { age: 31 }, "findOneAndUpdate returns the document after, projected");
const before = await c.findOneAndReplace({ name: "Di" }, { name: "Di", age: 41 });
is(before.age, 40, "findOneAndReplace returns the document before by default");
is((await c.findOneAndDelete({ name: "Di" })).age, 41, "findOneAndDelete");
is(await c.findOneAndDelete({ name: "Di" }), null, "and null when nothing matched");

is((await c.deleteOne({ age: { $gte: 0 } })).deletedCount, 1, "deleteOne deletes one");
is((await c.deleteMany({ senior: true })).deletedCount, 1, "deleteMany");

// Ordered bulk writes stop at the first error; unordered ones carry on.
await c.createIndex({ name: 1 }, { unique: true });
const ops = [
  { insertOne: { document: { name: "X" } } },
  { insertOne: { document: { name: "X" } } },
  { updateOne: { filter: { name: "X" }, update: { $set: { v: 1 } } } },
];
let ordered;
try {
  await c.bulkWrite(ops);
} catch (e) {
  ordered = e;
}
is(ordered?.code, "ERR_DB_UNIQUE_VIOLATION", "an ordered bulkWrite throws the duplicate");
is(ordered?.writeErrors?.[0]?.index, 1, "with the failing operation's position");
is(ordered?.result?.insertedCount, 1, "and what ran before it");
is((await c.findOne({ name: "X" })).v, undefined, "nothing after the error ran");
await c.deleteMany({ name: "X" });
let unordered;
try {
  await c.bulkWrite(ops, { ordered: false });
} catch (e) {
  unordered = e;
}
is(unordered?.result?.matchedCount, 1, "an unordered bulkWrite runs the rest");

const names = await c.createIndexes([{ key: { age: -1 } }, { key: { a: 1, b: 1 }, name: "ab" }]);
is(names, ["age_-1", "ab"], "createIndexes names its indexes");
ok(
  (await c.listIndexes().toArray()).some((i) => i.name === "ab"),
  "listIndexes",
);
await c.dropIndex("ab");
ok(!(await c.listIndexes().toArray()).some((i) => i.name === "ab"), "dropIndex");

const db = client.db();
const made = await db.createCollection("crud_made");
ok(
  (await db.listCollections({ name: "crud_made" }, { nameOnly: true }).toArray()).length === 1,
  "createCollection and listCollections",
);
is(await made.drop(), true, "drop");
is(await made.drop(), true, "and dropping a collection that is not there is not an error");
if (report("crud") > 0) throw new Error("crud failed");
