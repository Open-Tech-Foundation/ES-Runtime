// Runs the curated conformance suite under esrun:
//
//     esrun crates/runtime/conformance/run.js
//
// The gated signal is `conformance_suite_passes` in crates/runtime/src/lib.rs,
// which drives the same files and the same harness.js over a synchronous test
// driver. This runner is the second opinion: a real event loop, a real
// filesystem, and the real CLI, so anything whose behaviour depends on being
// driven for real — streams, dynamic imports, `runtime:fs`, the async
// WebAssembly paths — is exercised the way a user's program would exercise it.
//
// Exits non-zero on any failure, so it can be wired into a script.
import { file, Glob } from "runtime:fs";

const here = new URL(".", import.meta.url).pathname;

// The suite files are plain scripts calling global `test(...)`, not modules, so
// they are evaluated rather than imported — same shape as the WPT runner. The
// harness must go first: it defines the globals they call.
(0, eval)(await file(here + "harness.js").text());

const skip = new Set(["harness.js", "run.js"]);
const names = [];
for await (const name of new Glob("*.js").scan(here)) {
  if (!skip.has(name)) names.push(name);
}
names.sort();

for (const name of names) {
  (0, eval)(await file(here + name).text());
}
await __await_all();

const { pass, fail, todo, failures, fixed } = __results;
for (const f of failures) console.log(`FAIL ${f}`);
for (const f of fixed) console.log(`FIXED (promote this todo to test) ${f}`);
console.log(
  `\n${pass}/${pass + fail} assertions passing across ${names.length} files ` +
    `(${todo} known deviations)`,
);

if (fail > 0) throw new Error(`${fail} conformance failure(s)`);
if (fixed.length > 0) {
  throw new Error(`${fixed.length} todo case(s) now pass — promote them to test`);
}
