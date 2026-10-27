// A replica set: discovery from any member, operations routed by read
// preference, tag sets and max staleness, a direct connection, a set name that
// does not match, cursors that stay on their member, and a new primary after
// the old one steps down. Runs only when MONGODB_RS_URL names a replica set
// started by test/replica-set.sh (members tagged dc a, b, b).
import { connect } from "runtime:db";
import { env } from "runtime:process";
import { driver } from "../dist/index.js";
import { is, ok, report } from "./assert.mjs";

if (!env.MONGODB_RS_URL) {
  console.log("skipped: MONGODB_RS_URL is not set (test/replica-set.sh starts one)");
} else {
  const url = new URL(env.MONGODB_RS_URL);
  const members = ["localhost:27031", "localhost:27032", "localhost:27033"];
  // Seeded with one secondary-to-be: the rest are discovered from it.
  const client = await connect(`mongodb://${members[1]}/esrun_test?replicaSet=rs0`, { driver });
  const topology = () => client._topology;
  is(
    topology().type,
    "ReplicaSetWithPrimary",
    "one seed discovers the replica set and its primary",
  );
  is(topology().servers.length, 3, "and every member");

  const where = async (readPreference) => {
    const hello = await client.command({ hello: 1 }, { readPreference });
    return { me: hello.me, primary: hello.isWritablePrimary === true, dc: hello.tags?.dc };
  };
  ok((await where("primary")).primary, "primary goes to the primary");
  ok(!(await where("secondary")).primary, "secondary goes to a secondary");
  ok(
    !(await where({ mode: "secondaryPreferred" })).primary,
    "secondaryPreferred finds a secondary",
  );
  is(
    (await where({ mode: "secondary", tags: [{ dc: "x" }, { dc: "b" }] })).dc,
    "b",
    "the first tag set that matches is used",
  );
  ok(
    !(await where({ mode: "secondary", maxStalenessSeconds: 90 })).primary,
    "maxStalenessSeconds selects fresh secondaries",
  );

  let e = null;
  try {
    await where({ mode: "secondary", maxStalenessSeconds: 10 });
  } catch (error) {
    e = error;
  }
  is(e?.code, "ERR_DB_UNSUPPORTED", "maxStalenessSeconds below 90 is refused");

  const strict = await connect(env.MONGODB_RS_URL, { driver, serverSelectionTimeoutMS: 700 });
  e = null;
  const started = Date.now();
  try {
    // No member is in data center "z".
    await strict.command(
      { hello: 1 },
      { readPreference: { mode: "secondary", tags: [{ dc: "z" }] } },
    );
  } catch (error) {
    e = error;
  }
  is(e?.code, "ERR_DB_TIMEOUT", "no suitable server fails after serverSelectionTimeoutMS");
  ok(Date.now() - started >= 600, "having waited for one");
  await strict.close();

  // Writes go to the primary; a cursor's getMores stay on its member.
  const c = client.db().collection("replica");
  await c.drop({ writeConcern: { w: "majority" } });
  await c.insertMany(
    Array.from({ length: 20 }, (_, i) => ({ i })),
    { writeConcern: { w: 3 } },
  );
  is(
    (await c.find({}, { readPreference: "secondary", batchSize: 3 }).toArray()).length,
    20,
    "a cursor on a secondary reads every batch",
  );

  // A direct connection to a secondary reads there, and cannot write.
  const secondary = (await where("secondary")).me;
  const direct = await connect(`mongodb://${secondary}/esrun_test?directConnection=true`, {
    driver,
  });
  is(direct._topology.type, "Single", "directConnection=true is a single server");
  is(await direct.db().collection("replica").countDocuments(), 20, "and reads from a secondary");
  e = null;
  try {
    await direct.db().collection("replica").insertOne({ i: -1 });
  } catch (error) {
    e = error;
  }
  ok([10107, 13435].includes(e?.backendCode), "but cannot write there");
  await direct.close();

  e = null;
  try {
    await connect(`mongodb://${members[0]}/?replicaSet=other`, { driver });
  } catch (error) {
    e = error;
  }
  is(
    e?.message,
    "no seed is a member of replica set other",
    "a replica set name that does not match",
  );

  // The primary steps down; writes find the new one. The server refuses a step
  // down while no secondary has caught up (a previous run's may be recent), so
  // it is asked again until another member is primary.
  const before = (await where("primary")).me;
  for (let i = 0; i < 30; i++) {
    // Stepping down closes the primary's connections, so its reply is no guide.
    await client.command({ replSetStepDown: 30, secondaryCatchUpPeriodSecs: 10 }).catch(() => {});
    const now = await where("primary").catch(() => null);
    if (now !== null && now.me !== before) break;
    await new Promise((r) => setTimeout(r, 1000));
  }
  let wrote = false;
  for (let i = 0; i < 60 && !wrote; i++) {
    try {
      await c.insertOne({ after: true });
      wrote = true;
    } catch {
      await new Promise((r) => setTimeout(r, 500));
    }
  }
  ok(wrote, "writes succeed once a new primary is elected");
  ok((await where("primary")).me !== before, "and go to the new primary");
  await client.close();
  void url;
}
if (report("replica") > 0) throw new Error("replica failed");
