import Calc, { PI, square } from "mathkit";              // default + named from exports "."
import { sin0, cos0 } from "mathkit/trig";               // exports subpath (+ its re-export)
import double from "mathkit/fn/double";                  // exports wildcard subpath, default

const fail = (m) => { throw new Error("FAIL: " + m); };
if (PI !== 3.14) fail("pkg named");
if (square(3) !== 9) fail("pkg named fn");
if (new Calc().name() !== "Calc") fail("pkg default");
if (sin0 !== 0 || cos0 !== 1) fail("pkg subpath + re-export");
if (double(21) !== 42) fail("pkg wildcard subpath default");
console.log("PKG-SUITE-OK");
