import postgres from "../dist/index.js";
import { env } from "runtime:process";
import { connect, DbErrorCode } from "runtime:db";

// Set by test/tls-server.sh, which stands up a PostgreSQL with a certificate
// from a private authority — the shape every internal deployment has, and the
// one the public roots have never heard of.
const url = env.PG_TLS_URL;
const ca = env.PG_CA;
if (!url || !ca) {
  console.log("skipped: PG_TLS_URL/PG_CA not set (see test/tls-server.sh)");
} else {
  // Without naming the authority, a certificate it signed is refused — which is
  // the half that proves the option grants trust rather than skipping the check.
  try {
    await connect(`${url}?sslmode=require`, { driver: postgres });
    console.log("unnamed CA: connected (should not happen)");
  } catch (e) {
    console.log("unnamed CA refused:", e.code === DbErrorCode.Unsupported || /certificate|unknown|Tls/i.test(e.message));
  }

  const db = await connect(`${url}?sslmode=require`, { driver: postgres, sslRootCert: ca });
  const row = await (await db.query("SELECT ssl, version FROM pg_stat_ssl WHERE pid = pg_backend_pid()")).first();
  console.log("named CA connected:", row.ssl, "|", row.version);
  await db.close();

  // The default (prefer) reaches the same server and also ends up encrypted.
  const preferred = await connect(url, { driver: postgres, sslRootCert: ca });
  const p = await (await preferred.query("SELECT ssl FROM pg_stat_ssl WHERE pid = pg_backend_pid()")).first();
  console.log("prefer encrypted:", p.ssl);
  await preferred.close();
}
