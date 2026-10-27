// A type test for `runtime:db`'s MongoDB driver (D186). Compiled by `tsc -p .`, never run.

import type { MongoCollection, UpdateResult } from "runtime:db";
import { connect, mongodb } from "runtime:db";

interface User {
  name: string;
  age?: number;
}

const client = await connect("mongodb://localhost/app", { driver: mongodb, maxPoolSize: 10 });
const users: MongoCollection<User> = client.db().collection<User>("users");
const one: unknown = (await users.insertOne({ name: "Ada" })).insertedId;
const found: User | null = await users.findOne({ name: "Ada" });
const updated: UpdateResult = await users.updateOne(
  { name: "Ada" },
  { $set: { age: 36 } },
  { upsert: true },
);
const all: User[] = await users
  .find({ age: { $gt: 30 } })
  .sort({ age: -1 })
  .limit(10)
  .toArray();
for await (const user of users.find()) void user.name;
const n: number = await users.countDocuments({}, { signal: AbortSignal.timeout(1000) });
const reply = await client.command({ ping: 1 }, { db: "admin", timeoutMS: 500 });
// @ts-expect-error — a MongoDB option the driver does not take.
await connect("mongodb://localhost/app", { driver: mongodb, maxPoolSize: "ten" });
// @ts-expect-error — insertMany takes an array.
await users.insertMany({ name: "Bo" });
void [one, found, updated, all, n, reply];
