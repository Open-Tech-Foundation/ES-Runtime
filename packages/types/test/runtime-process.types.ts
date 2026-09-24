// A type test for `runtime:process`'s timer references. Compiled by
// `tsc -p .`, never run.

import { refTimer, unrefTimer } from "runtime:process";

const heartbeat = setInterval(() => {}, 1000);
unrefTimer(heartbeat);
refTimer(heartbeat);
unrefTimer(setTimeout(() => {}, 10));

// @ts-expect-error — a timer id, as setTimeout returned it.
unrefTimer("heartbeat");
