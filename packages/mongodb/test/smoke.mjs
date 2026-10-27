// The package's exports, end to end: connect with `driver`, write and read a
// document carrying BSON types, close.
import { connect } from "runtime:db";
import { env } from "runtime:process";
import { BSON, Decimal128, driver, ObjectId } from "../dist/index.js";
import { is, ok, report } from "./assert.mjs";

const client = await connect(env.MONGODB_URL, { driver });
is(client.backend, "mongodb", "the backend names itself");
await client.ping();
ok(ObjectId === BSON.ObjectId, "the package's ObjectId is runtime:serialization's");

const things = client.db().collection("smoke");
await things.drop();
const id = new ObjectId();
await things.insertOne({ _id: id, price: new Decimal128("9.99"), at: new Date(0), n: 5n });
const back = await things.findOne({ _id: id });
ok(back._id instanceof ObjectId && back._id.equals(id), "the _id comes back an ObjectId");
is(back.price.toString(), "9.99", "a Decimal128 keeps its digits");
is(String(back.n), "5", "an int64 keeps its value");
is(typeof back.n, "bigint", "an int64 reads as a bigint");
is(back.at.getTime(), 0, "a Date round-trips");
is(client.db().databaseName, "esrun_test", "db() is the connection string's database");

await client.close();
let closed = null;
try {
  client.db();
} catch (e) {
  closed = e.code;
}
is(closed, "ERR_DB_CLOSED", "a closed client says so");
if (report("smoke") > 0) throw new Error("smoke failed");
