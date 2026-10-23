// Server families: one RESP implementation under another server's names. What
// a family changes is checkable without a server — which URLs it takes, what
// its drivers and connections are called, what its errors say.
import { connect, DbErrorCode } from "runtime:db";
import { exit } from "runtime:process";
import {
  createClusterDriver,
  createDriver,
  createSentinelDriver,
  defineFamily,
  driver,
  parseConnectionString,
  REDIS,
  REDIS_DIALECT,
  RedisConnection,
  redisCluster,
  redisSentinel,
} from "../../dist/index.js";
import { is, ok, report, throws } from "./assert.mjs";

// -- Redis is the default, and unchanged ------------------------------------

is(driver.name, "redis", "the Redis driver keeps its name");
is(driver.schemes, ["redis", "rediss"], "and its schemes");
is(redisCluster.name, "redis-cluster", "the cluster driver too");
is(redisSentinel.name, "redis-sentinel", "and the Sentinel driver");
ok(REDIS.dialect === REDIS_DIALECT, "REDIS_DIALECT is the Redis family's dialect");
ok(createDriver(REDIS) === driver, "the Redis family's driver is the one exported as driver");
is(new RedisConnection().backend, "redis", "a connection is Redis unless told otherwise");

{
  let message = "";
  try {
    parseConnectionString("valkey://host");
  } catch (e) {
    message = e.message;
  }
  is(
    message,
    "valkey: is not a redis scheme — use redis:// or rediss://",
    "the Redis family names its own schemes",
  );
}

// -- another family ----------------------------------------------------------

const ACME = defineFamily({
  name: "acme",
  schemes: ["acme", "acmes", "redis", "rediss"],
  tlsSchemes: ["acmes", "rediss"],
});

ok(parseConnectionString("acme://host", {}, ACME).tls !== true, "its plain scheme is plaintext");
ok(parseConnectionString("acmes://host", {}, ACME).tls === true, "its TLS scheme is TLS");
ok(parseConnectionString("rediss://host", {}, ACME).tls === true, "a borrowed TLS scheme too");
ok(parseConnectionString("redis://host", {}, ACME).tls !== true, "and a borrowed plain one is not");
is(parseConnectionString("acme://host:7000/2", {}, ACME).port, 7000, "the rest parses as before");
is(parseConnectionString("acme://host:7000/2", {}, ACME).db, 2, "including the database index");

{
  let error = null;
  try {
    parseConnectionString("other://host", {}, ACME);
  } catch (e) {
    error = e;
  }
  is(error?.code, DbErrorCode.Unsupported, "a scheme outside the family is refused");
  is(
    error?.message,
    "other: is not a acme scheme — use acme://, acmes://, redis:// or rediss://",
    "naming every scheme it would take",
  );
}

{
  let message = "";
  try {
    parseConnectionString("acme://host/name", {}, ACME);
  } catch (e) {
    message = e.message;
  }
  ok(message.startsWith("the path of a acme: URL"), "path errors name the family");
}

const acme = createDriver(ACME);
ok(createDriver(ACME) === acme, "a family has one driver, whoever asks for it");
is(acme.name, "acme", "named after the family");
is(acme.schemes, ["acme", "acmes", "redis", "rediss"], "taking every scheme it has");
ok(acme.dialect === ACME.dialect, "with the family's dialect");
is(ACME.dialect.name, "acme", "which is named after it");
{
  let message = "";
  try {
    ACME.dialect.placeholder(1);
  } catch (e) {
    message = e.message;
  }
  ok(message.startsWith("the acme backend has no SQL"), "and whose refusals say acme");
}
is(createClusterDriver(ACME).name, "acme-cluster", "its cluster driver");
is(createClusterDriver(ACME).schemes, acme.schemes, "takes the same schemes");
is(createSentinelDriver(ACME).name, "acme-sentinel", "and so does its Sentinel driver");
is(new RedisConnection(ACME).backend, "acme", "a connection reports the family as its backend");

// runtime:db checks the scheme against the driver before it opens anything,
// so this needs no server: the Redis driver does not take the other family's.
{
  let code = null;
  try {
    await connect("acme://127.0.0.1:1", { driver });
  } catch (e) {
    code = e.code;
  }
  is(code, DbErrorCode.Unsupported, "the Redis driver refuses another family's scheme");
}

// -- what a family must have -------------------------------------------------

await throws(
  () => defineFamily({ name: "x", schemes: ["xs", "x"], tlsSchemes: ["xs"] }),
  "the first scheme must be a plaintext one — internal URLs are built with it",
);
await throws(
  () => defineFamily({ name: "x", schemes: ["x"], tlsSchemes: [] }),
  "and a family with no TLS scheme is refused",
);

exit(report("family") === 0 ? 0 : 1);
