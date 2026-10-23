// The Valkey family's names, which is everything this package adds — checkable
// without a server.
import { connect, DbErrorCode } from "runtime:db";
import { exit } from "runtime:process";
import { driver as redis } from "@opentf/esrun-redis";
import {
  driver,
  parseConnectionString,
  VALKEY,
  ValkeyConnection,
  valkeyCluster,
  valkeySentinel,
} from "@opentf/esrun-valkey";
import { is, ok, report } from "./assert.mjs";

is(driver.name, "valkey", "the driver is named valkey");
is(driver.schemes, ["valkey", "valkeys", "redis", "rediss"], "and takes both families' schemes");
is(valkeyCluster.name, "valkey-cluster", "the cluster driver");
is(valkeySentinel.name, "valkey-sentinel", "the Sentinel driver");
is(valkeyCluster.schemes, driver.schemes, "which take the same schemes");
is(VALKEY.dialect.name, "valkey", "the dialect is Valkey's");
is(new ValkeyConnection(VALKEY).backend, "valkey", "a connection reports valkey as its backend");

ok(parseConnectionString("valkey://host").tls !== true, "valkey:// is plaintext");
ok(parseConnectionString("valkeys://host").tls === true, "valkeys:// is TLS from the first byte");
ok(parseConnectionString("redis://host").tls !== true, "redis:// is taken, and plaintext");
ok(parseConnectionString("rediss://host").tls === true, "rediss:// is taken, and TLS");
is(parseConnectionString("valkey://host").port, 6379, "the default port is Redis's");
is(parseConnectionString("valkey://:pw@host/3").db, 3, "the path is a database index");
is(parseConnectionString("valkey://:pw@host/3").password, "pw", "a password in the userinfo");

{
  let error = null;
  try {
    parseConnectionString("memcached://host");
  } catch (e) {
    error = e;
  }
  is(error?.code, DbErrorCode.Unsupported, "another scheme is refused");
  is(
    error?.message,
    "memcached: is not a valkey scheme — use valkey://, valkeys://, redis:// or rediss://",
    "naming the schemes that would work",
  );
}

{
  let message = "";
  try {
    parseConnectionString("valkey://host?password=x");
  } catch (e) {
    message = e.message;
  }
  ok(message.includes("valkey://:password@host"), "the password hint is spelled valkey://");
}

// runtime:db checks the scheme before the driver opens anything, so these need
// no server. The Redis driver keeps to Redis's schemes; this package is where
// valkey:// is taken.
{
  let code = null;
  try {
    await connect("valkey://127.0.0.1:1", { driver: redis });
  } catch (e) {
    code = e.code;
  }
  is(code, DbErrorCode.Unsupported, "the Redis driver does not take valkey://");
}
{
  let code = null;
  try {
    await connect("postgres://127.0.0.1:1", { driver });
  } catch (e) {
    code = e.code;
  }
  is(code, DbErrorCode.Unsupported, "and the Valkey driver does not take another database's");
}

exit(report("url") === 0 ? 0 : 1);
