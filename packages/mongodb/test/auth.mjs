// Authentication: SCRAM-SHA-256 and SCRAM-SHA-1, negotiated or named, a wrong
// password refused, and a user whose name and password need escaping.
// Runs only when MONGODB_AUTH_URL names a server with access control on.
import { connect } from "runtime:db";
import { env, unmask } from "runtime:process";
import { driver } from "../dist/index.js";
import { is, report } from "./assert.mjs";

if (!env.MONGODB_AUTH_URL) {
  console.log("skipped: MONGODB_AUTH_URL is not set");
} else {
  // A variable named like a credential arrives masked.
  const adminUrl = String(unmask(env.MONGODB_AUTH_URL));
  const admin = await connect(adminUrl, { driver });
  const url = new URL(adminUrl);
  const at = `${url.hostname}:${url.port || 27017}`;
  const user = "esrun:user@x";
  const password = "p@ss/wörd";
  await admin.command({ dropUser: user }).catch(() => {});
  await admin.command({ createUser: user, pwd: password, roles: [] });

  const encoded = `mongodb://${encodeURIComponent(user)}:${encodeURIComponent(password)}@${at}/?authSource=admin`;
  for (const mechanism of [null, "SCRAM-SHA-256", "SCRAM-SHA-1"]) {
    const client = await connect(mechanism ? `${encoded}&authMechanism=${mechanism}` : encoded, {
      driver,
    });
    const status = await client.command({ connectionStatus: 1 });
    is(
      status.authInfo.authenticatedUsers[0]?.user,
      user,
      `authenticated with ${mechanism ?? "the negotiated mechanism"}`,
    );
    await client.close();
  }
  const viaOptions = await connect(`mongodb://${at}/`, {
    driver,
    auth: { username: user, password },
  });
  await viaOptions.ping();
  await viaOptions.close();
  is(true, true, "credentials given as options");

  let code = null;
  try {
    await connect(`mongodb://${encodeURIComponent(user)}:wrong@${at}/`, { driver });
  } catch (e) {
    code = e.code;
  }
  is(code, "ERR_DB_AUTH_FAILED", "a wrong password is refused");
  await admin.command({ dropUser: user });
  await admin.close();
}
if (report("auth") > 0) throw new Error("auth failed");
