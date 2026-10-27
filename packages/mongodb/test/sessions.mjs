// Sessions: every command carries one, the server sees it, server sessions are
// reused newest first, a cursor keeps its session until it is exhausted, and a
// session is refused where the specification says it must be.
// Needs a server started with enableTestCommands, for `sleep`.
import { connect } from "runtime:db";
import { env } from "runtime:process";
import { driver } from "../dist/index.js";
import { is, ok, report } from "./assert.mjs";

await using client = await connect(env.MONGODB_URL, { driver });
const pooled = () => client._sessions._queue.length;

// The server sees the explicit session's id on the command it runs.
const session = client.startSession();
const sleeping = client.command({ sleep: 1, millis: 400 }, { session });
await new Promise((r) => setTimeout(r, 100));
// Matched by command and duration: an earlier test's longer sleep may still be
// running on the server after its client gave up on it.
const ops = await client
  .db("admin")
  .aggregate([
    { $currentOp: { idleSessions: false } },
    { $match: { "command.sleep": 1, "command.millis": 400 } },
  ])
  .toArray();
const op = ops[0];
await sleeping;
ok(op?.lsid?.id?.equals(session.id.id), "the server sees the session's lsid on its command");

// Implicit sessions are reused: sequential commands share one server session.
await session.endSession();
const start = pooled();
for (let i = 0; i < 20; i++) await client.ping();
is(pooled(), Math.max(start, 1), "sequential commands reuse one server session");
await Promise.all([client.ping(), client.ping(), client.ping()]);
ok(pooled() >= 3, "concurrent commands each had their own");

// A cursor holds its implicit session until it is exhausted.
const c = client.db().collection("sessions");
await c.drop();
await c.insertMany(Array.from({ length: 10 }, (_, i) => ({ i })));
const before = pooled();
const cursor = c.find({}, { batchSize: 2 });
await cursor.next();
is(pooled(), before - 1, "an open cursor holds a server session");
await cursor.toArray();
is(pooled(), before, "and returns it once exhausted");
const left = c.find({}, { batchSize: 2 });
await left.next();
await left.close();
is(pooled(), before, "or closed");

// An explicit session works across operations and ends once.
await using explicit = client.startSession();
await c.insertOne({ i: 99 }, { session: explicit });
is(
  (await c.findOne({ i: 99 }, { session: explicit })).i,
  99,
  "an explicit session carries reads and writes",
);
await explicit.endSession();
await explicit.endSession();
let refused = null;
try {
  await c.findOne({}, { session: explicit });
} catch (e) {
  refused = e.message;
}
is(refused, "the session has ended", "an ended session is refused");

await using other = await connect(env.MONGODB_URL, { driver });
const foreign = other.startSession();
refused = null;
try {
  await c.findOne({}, { session: foreign });
} catch (e) {
  refused = e.message;
}
is(refused, "the session belongs to another client", "another client's session is refused");
await foreign.endSession();

const unack = client.startSession();
refused = null;
try {
  await c.insertOne({ i: 1 }, { session: unack, writeConcern: { w: 0 } });
} catch (e) {
  refused = e.message;
}
is(
  refused,
  "an unacknowledged write (w: 0) cannot use a session",
  "w: 0 refuses an explicit session",
);
await unack.endSession();
await c.drop();
if (report("sessions") > 0) throw new Error("sessions failed");
