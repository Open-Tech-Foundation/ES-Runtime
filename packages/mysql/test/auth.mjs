// Logging in: both password plugins' paths, a wrong password, and TLS refusing
// a certificate it cannot verify.
import { connect, DbErrorCode } from "runtime:db";
import { env } from "runtime:process";
import { driver } from "../dist/index.js";
import { is, ok, report } from "./unit/assert.mjs";

const url = env.MYSQL_URL ?? "mysql://root:esrun@127.0.0.1:3307/esrun_test?ssl-mode=DISABLED";
const at = new URL(url);
const as = (user, password, extra = "ssl-mode=DISABLED") =>
  `mysql://${user}:${encodeURIComponent(password)}@${at.host}${at.pathname}?${extra}`;
const admin = await connect(url, { driver });

// A wrong password is an authentication failure, not a lost connection.
let code = null;
try {
  await connect(as("root", "not-the-password"), { driver });
} catch (e) {
  code = e.code;
}
is(code, DbErrorCode.AuthFailed, "a wrong password");

// caching_sha2_password with nothing cached: the server wants the password
// itself, and over plaintext it is RSA-encrypted to the server's key. MySQL's
// plugin; MariaDB has none by that name.
if (!admin.mariadb) {
  await admin.executeScript(`
    DROP USER IF EXISTS 'esrun_sha2'@'%';
    CREATE USER 'esrun_sha2'@'%' IDENTIFIED WITH caching_sha2_password BY 'pässword-2';
    GRANT SELECT ON *.* TO 'esrun_sha2'@'%';
    FLUSH PRIVILEGES;
  `);
  const full = await connect(as("esrun_sha2", "pässword-2"), { driver });
  is(
    (await (await full.query("SELECT CURRENT_USER() AS u")).first()).u,
    "esrun_sha2@%",
    "full authentication over RSA",
  );
  await full.close();
  const fast = await connect(as("esrun_sha2", "pässword-2"), { driver });
  is((await (await fast.query("SELECT 1 AS one")).first()).one, 1, "then the cached fast path");
  await fast.close();
  await admin.execute("DROP USER 'esrun_sha2'@'%'");
} else {
  console.log("  skip caching_sha2_password: MariaDB has no such plugin");
}

// An account with an empty password.
await admin.executeScript(`
  DROP USER IF EXISTS 'esrun_empty'@'%';
  CREATE USER 'esrun_empty'@'%' IDENTIFIED BY '';
  GRANT SELECT ON esrun_test.* TO 'esrun_empty'@'%';
`);
const empty = await connect(`mysql://esrun_empty@${at.host}${at.pathname}?ssl-mode=DISABLED`, {
  driver,
});
is((await (await empty.query("SELECT 1 AS one")).first()).one, 1, "an empty password");
await empty.close();

// mysql_native_password, where the server still has it (8.4 ships it disabled).
const native = await (
  await admin.query(
    "SELECT PLUGIN_STATUS AS s FROM information_schema.plugins WHERE PLUGIN_NAME = 'mysql_native_password'",
  )
).first();
if (native?.s === "ACTIVE") {
  await admin.executeScript(`
    DROP USER IF EXISTS 'esrun_native'@'%';
    CREATE USER 'esrun_native'@'%' IDENTIFIED ${admin.mariadb ? "" : "WITH mysql_native_password "}BY 'native-pw';
    GRANT SELECT ON esrun_test.* TO 'esrun_native'@'%';
  `);
  const n = await connect(as("esrun_native", "native-pw"), { driver });
  is((await (await n.query("SELECT 1 AS one")).first()).one, 1, "mysql_native_password");
  await n.close();
  await admin.execute("DROP USER 'esrun_native'@'%'");
} else {
  console.log("  skip mysql_native_password: the server has it disabled");
}

// A stock server's certificate is self-signed: TLS must refuse it, and say what to do.
let tls = null;
try {
  await connect(
    as("root", at.password ? decodeURIComponent(at.password) : "", "ssl-mode=REQUIRED"),
    { driver },
  );
} catch (e) {
  tls = e;
}
ok(
  tls !== null && /sslRootCert/.test(tls.message),
  `an unverifiable certificate is refused with the fix named (${tls?.message})`,
);

await admin.execute("DROP USER 'esrun_empty'@'%'");
await admin.close();
if (report("auth") > 0) throw new Error("auth failed");
