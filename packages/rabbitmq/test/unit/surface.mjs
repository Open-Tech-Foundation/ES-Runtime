// The public surface, pinned.
//
// `src/index.ts` is the whole of what a consumer can reach: only the package
// root is exported (`package.json`'s `exports`), so a name missing here is a
// name no one can import. The type-only exports are erased at runtime and
// asserted by `esdev typecheck` instead; what is checked here is that every
// value the package promises is reachable from the root, and that the root pulls
// in no capability — importing it must not, by itself, need `--allow-net`.
import { exit } from "runtime:process";
import * as rabbitmq from "../../dist/index.js";
import { is, ok, report } from "./assert.mjs";

const VALUES = [
  "Channel",
  "Connection",
  "DEFAULT_HEARTBEAT",
  "DEFAULT_PREFETCH",
  "DIRECT_REPLY_TO",
  "Pool",
  "ProtocolError",
  "RabbitError",
  "RabbitErrorCode",
  "RecoveringChannel",
  "RecoveringSubscription",
  "Session",
  "openSession",
  "RpcClient",
  "Subscription",
  "netTransport",
  "openConnection",
  "parseAmqpUrl",
  "subscribe",
];

for (const name of VALUES) {
  ok(name in rabbitmq, `the package root exports ${name}`);
}

ok(typeof rabbitmq.RabbitErrorCode.Closed === "string", "the error codes are values");
is(
  rabbitmq.parseAmqpUrl("amqp://guest:guest@localhost/").vhost,
  "/",
  "the URL parser works through the root",
);
ok(rabbitmq.DIRECT_REPLY_TO.startsWith("amq."), "and the direct reply-to constant is there");

if (report("surface") > 0) exit(1);
