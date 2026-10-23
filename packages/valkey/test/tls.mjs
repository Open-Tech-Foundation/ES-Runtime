// valkeys:// — TLS from the first byte, against a private authority.
//
// Needs a TLS server; see ../redis/test/tls-server.sh. Skipped without one,
// because a test that quietly passes when it did not run is worse than no test.
import { connect } from "runtime:db";
import { env, exit } from "runtime:process";
import { driver } from "@opentf/esrun-valkey";
import { is, report } from "./unit/assert.mjs";

const url = env.VALKEY_TLS_URL;
const ca = env.VALKEY_CA;
if (!url || !ca) {
  console.log("skip tls — set VALKEY_TLS_URL and VALKEY_CA (see test/run.sh)");
  exit(0);
}

const v = await connect(url, { driver, tlsCa: ca });
is(await v.ping(), "PONG", "valkeys:// with a private CA connects");
is(v.hello.server, "valkey", "to Valkey");
await v.close();

exit(report("tls") === 0 ? 0 : 1);
