// MongoDB's initial DNS seedlist discovery tests (the replica-set directory),
// against the public records under test.build.10gen.cc and the TLS replica set
// test/srv-replica-set.sh starts. Runs only when MONGODB_SRV_CA names that
// script's CA, and the specification checkout (mongodb-specs/fetch.sh) exists.
//
// Each test is a mongodb+srv:// URL and what it must produce: an error, or the
// seeds DNS gives, the options the URL and the TXT record settle on, the
// replica set's hosts, and a ping.
import { connect } from "runtime:db";
import { file, readDir } from "runtime:fs";
import { env } from "runtime:process";
import { driver } from "../dist/index.js";
import { is, ok, report } from "./assert.mjs";

const dir = new URL(
  "../../../mongodb-specs/upstream/source/initial-dns-seedlist-discovery/tests/replica-set/",
  import.meta.url,
).pathname;

// Options esrun does not offer (D188): each relaxes the domain check.
const NOT_OFFERED = /srvAllowedHostsSuffix/;

/// The settings a spec option name is read back from.
const OPTION = {
  ssl: (s) => s.tls,
  replicaSet: (s) => s.replicaSet,
  authSource: (s) => s.authSource,
  srvMaxHosts: (s) => s.srv.maxHosts,
  srvServiceName: (s) => s.srv.serviceName,
  directConnection: (s) => s.directConnection,
  loadBalanced: (s) => s.loadBalanced ?? false,
};

const PARSED = {
  user: (s) => s.user,
  password: (s) => s.password,
  db: (s) => s.database,
  defaultDatabase: (s) => s.database,
  auth_database: (s) => s.authSource,
};

if (!env.MONGODB_SRV_CA) {
  console.log(
    "skipped: MONGODB_SRV_CA is not set (test/srv-replica-set.sh starts the replica set and prints it)",
  );
} else {
  const ca = await file(env.MONGODB_SRV_CA).text();
  const names = (await readDir(dir))
    .map((e) => e.name)
    .filter((n) => n.endsWith(".json"))
    .sort();
  let skipped = 0;
  for (const name of names) {
    if (NOT_OFFERED.test(name)) {
      skipped++;
      continue;
    }
    const test = JSON.parse(await file(dir + name).text());
    let client;
    try {
      client = await connect(test.uri, { driver, ca });
    } catch (error) {
      // Credentials are checked when connecting, and this replica set has no
      // users: a test that only parses them sees the server refuse them.
      const refused = test.parsed_options?.user !== undefined && /auth/i.test(error.message);
      // A test of discovery alone names seeds no server listens on, and
      // connect() fails when no seed answers: it must have tried just those.
      const unanswered =
        test.ping === false &&
        test.hosts === undefined &&
        /^no server answered/.test(error.message) &&
        test.seeds.every((seed) => error.message.includes(`${seed}:`)) &&
        error.message.split(";").length === test.seeds.length;
      ok(test.error === true || refused || unanswered, `${name}: ${error.message}`);
      continue;
    }
    try {
      ok(test.error !== true, `${name}: connected, but the test expects an error`);
      const settings = client._settings;
      if (test.seeds !== undefined)
        is([...settings.hosts].sort(), [...test.seeds].sort(), `${name}: seeds`);
      if (test.numSeeds !== undefined)
        is(settings.hosts.length, test.numSeeds, `${name}: number of seeds`);
      for (const [option, want] of Object.entries(test.options ?? {})) {
        const read = OPTION[option];
        ok(read !== undefined, `${name}: the runner reads option ${option}`);
        if (read) is(read(settings), want, `${name}: option ${option}`);
      }
      for (const [option, want] of Object.entries(test.parsed_options ?? {})) {
        is(PARSED[option]?.(settings), want, `${name}: parsed ${option}`);
      }
      if (test.ping !== false) await client.ping();
      if (test.hosts !== undefined && test.hosts.length > 0) {
        await client.ping();
        const hosts = client._topology.servers.map((server) => server.address).sort();
        is(hosts, [...test.hosts].sort(), `${name}: hosts`);
      }
    } finally {
      await client.close();
    }
  }
  console.log(
    `  ${names.length - skipped} tests run, ${skipped} skipped (srvAllowedHostsSuffix is not offered)`,
  );
}
if (report("srv") > 0) throw new Error("srv failed");
