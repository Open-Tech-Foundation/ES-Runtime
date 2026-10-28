// A type test for `runtime:process`'s `environment()` (D192). Compiled by
// `tsc -p .`, never run.

import process, { environment } from "runtime:process";

const stage: string = environment();
const same: string = process.environment();
// @ts-expect-error — it takes nothing: the names it reads are fixed.
environment("RUNTIME_ENV");
void [stage, same];
