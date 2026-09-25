// TLS against a server whose certificate a private authority signed.
import { connect } from "runtime:db";
import { env } from "runtime:process";
import { driver } from "../dist/index.js";
import { is, ok, report } from "./unit/assert.mjs";

// Set by test/tls-server.sh. The server requires secure transport, so a login
// that succeeds at all has negotiated TLS.
const url = env.MYSQL_TLS_URL;
const ca = env.MYSQL_CA;
if (!url || !ca) {
  console.log("skipped: MYSQL_TLS_URL/MYSQL_CA not set (see test/tls-server.sh)");
} else {
  let refused = null;
  try {
    await connect(`${url}?ssl-mode=REQUIRED`, { driver });
  } catch (e) {
    refused = e;
  }
  ok(refused !== null, "without naming the authority, its certificate is refused");

  const db = await connect(`${url}?ssl-mode=REQUIRED`, { driver, sslRootCert: ca });
  const row = await (await db.query("SHOW SESSION STATUS LIKE 'Ssl_version'")).first();
  ok(/^TLSv1\.[23]$/.test(row.Value), `named, it connects over TLS (${row.Value})`);
  // Over TLS, caching_sha2's full authentication sends the password as is.
  await db.executeScript(`
    DROP USER IF EXISTS 'esrun_tls'@'%';
    CREATE USER 'esrun_tls'@'%' IDENTIFIED BY 'tls-pw';
    GRANT SELECT ON esrun_test.* TO 'esrun_tls'@'%';
    FLUSH PRIVILEGES;
  `);
  const user = new URL(url);
  user.username = "esrun_tls";
  user.password = "tls-pw";
  const other = await connect(`${user.href}?ssl-mode=REQUIRED`, { driver, sslRootCert: ca });
  is(
    (await (await other.query("SELECT CURRENT_USER() AS u")).first()).u,
    "esrun_tls@%",
    "full authentication over TLS",
  );
  await other.close();
  await db.execute("DROP USER 'esrun_tls'@'%'");
  await db.close();
  if (report("tls") > 0) throw new Error("tls failed");
}
