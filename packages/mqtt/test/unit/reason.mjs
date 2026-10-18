// Reason codes: the one test that matters is where success stops and refusal
// begins, and that a name comes back for the codes the specification spells.
import { exit } from "runtime:process";
import { connack311Name, isErrorReason, reasonName } from "../../dist/protocol/reason.js";
import { ok, report } from "./assert.mjs";

ok(isErrorReason(0x00) === false, "Success is not an error");
ok(isErrorReason(0x02) === false, "Granted QoS 2 is not an error");
ok(isErrorReason(0x10) === false, "No matching subscribers is not an error");
ok(isErrorReason(0x7f) === false, "everything below 0x80 is not an error");
ok(isErrorReason(0x80) === true, "Unspecified error is an error");
ok(isErrorReason(0x87) === true, "Not authorized is an error");
ok(isErrorReason(0xff) === true, "everything at or above 0x80 is an error");

ok(reasonName(0x00) === "Success", "0x00 is Success");
ok(reasonName(0x01) === "Granted QoS 1", "0x01 is Granted QoS 1");
ok(reasonName(0x10) === "No matching subscribers", "0x10 is No matching subscribers");
ok(reasonName(0x87) === "Not authorized", "0x87 is Not authorized");
ok(reasonName(0x8f) === "Topic Filter invalid", "0x8f is Topic Filter invalid");
ok(reasonName(0xa2) === "Wildcard Subscriptions not supported", "0xa2 is named");
ok(reasonName(0x03) === null, "an unassigned code has no name");
ok(reasonName(0x99) === "Payload format invalid", "0x99 is Payload format invalid");

ok(connack311Name(0) === "Connection Accepted", "3.1.1 accepted");
ok(connack311Name(2) === "Identifier Rejected", "3.1.1 identifier rejected");
ok(connack311Name(5) === "Not authorized", "3.1.1 not authorized");
ok(connack311Name(6) === null, "a 3.1.1 code past the set has no name");

exit(report("reason") > 0 ? 1 : 0);
