import { env } from "runtime:process";
import { driver as postgres, parseConnectionString, environmentDefaults } from "../dist/index.js";
import { connect } from "runtime:db";

const show = (o) => JSON.stringify({ host: o.host, port: o.port, user: o.user, database: o.database, sslmode: o.sslmode, connectTimeout: o.connectTimeout, applicationName: o.applicationName });

// The environment fills what the URL left out.
console.log("env seen:", JSON.stringify(environmentDefaults()));
console.log("bare url:", show(parseConnectionString("postgres://")));

// The URL wins over the environment — a program that spelled out a host should
// get that host whatever the shell exported.
console.log("url wins:", show(parseConnectionString("postgres://someone@elsewhere:6000/other")));

// Explicit options win over both.
console.log("options win:", show(parseConnectionString("postgres://someone@elsewhere:6000/other", { host: "explicit", port: 1 })));

// Reading the environment needs the Env capability, and the driver treats a
// refusal as "no defaults" rather than as a failure — a connection string that
// named everything it needed should still work with nothing else granted.
let granted = false;
try {
  granted = Boolean(env.PGHOST);
} catch {
  console.log("env denied, defaults empty:", JSON.stringify(environmentDefaults()) === "{}");
}

// And it actually connects using only the environment.
if (granted) {
  const db = await connect("postgres://", { driver: postgres });
  console.log("connected from env:", (await (await db.query("SELECT 5 AS n")).first()).n);
  await db.close();
}
