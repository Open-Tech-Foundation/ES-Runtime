// Exercises every standardized ESM import form against the export fixtures,
// asserting each. Prints ESM-SUITE-OK only if all pass (otherwise throws).

// Import patterns:
import { PI, add, Calculator } from "./exporter.mjs"; // 1,2 named (multiple)
import { TAU_ALIAS as TAU } from "./exporter.mjs"; //      3 named with alias
import makeApp from "./exporter.mjs"; //                   4 default
import maker, { PI as PI_again, E } from "./exporter.mjs"; // 5 default + named
import * as ExporterNS from "./exporter.mjs"; //           6 namespace
import "./side-effect.mjs"; //                             7 side-effect

import defaultAlias, { tag } from "./default-alias.mjs"; // export { x as default }

// Consume the re-export module (export patterns 7–12):
import reDefault, {
  PI as RPI,
  add as Radd,
  PI2,
  add2,
  Maker,
  x,
  y,
  more,
} from "./reexporter.mjs";

const fail = (m) => {
  throw new Error("FAIL: " + m);
};

if (PI !== 3.14159) fail("named PI");
if (add(2, 3) !== 5) fail("named add");
if (new Calculator().kind() !== "calc") fail("named class");
if (TAU !== 6.283) fail("named alias TAU");
if (typeof makeApp !== "function" || makeApp().name !== "app") fail("default export");
if (maker !== makeApp) fail("default identity across imports");
if (PI_again !== 3.14159 || E !== 2.718) fail("default + named imports");
if (ExporterNS.PI !== 3.14159 || typeof ExporterNS.default !== "function") fail("namespace import");
if (globalThis.__sideEffectRan !== true) fail("side-effect import");
if (defaultAlias() !== "hi" || tag !== "da") fail("export { x as default }");

if (RPI !== 3.14159 || Radd(1, 1) !== 2) fail("re-export named");
if (PI2 !== 3.14159 || add2(1, 1) !== 2) fail("re-export with alias");
if (typeof reDefault !== "function" || reDefault().name !== "app") fail("re-export default");
if (typeof Maker !== "function") fail("re-export default as named");
if (x !== 10 || y !== 20) fail("export * (re-export all)");
if (more.x !== 10 || more.y !== 20) fail("export * as namespace");

// Dynamic import forms:
const dyn = await import("./exporter.mjs"); //                      8 dynamic
if (dyn.PI !== 3.14159) fail("dynamic import");
const { add: dynAdd } = await import("./exporter.mjs"); //          9 destructured
if (dynAdd(3, 4) !== 7) fail("dynamic import destructured");
const { default: DynMaker } = await import("./exporter.mjs"); //   10 default via destructure
if (DynMaker().name !== "app") fail("dynamic import default");

console.log("ESM-SUITE-OK");
