// Runs the vendored WPT urlpattern suite under esrun:
//
//     esrun crates/runtime/wpt/urlpattern/run.js
//
// Deliberately NOT wired into `cargo test`: a full WPT subset is post-1.0
// (SPEC §7), and this is a reference check rather than a release gate. The
// gated signal for URLPattern is conformance/urlpattern.js.
import { file } from "runtime:fs";

const here = new URL(".", import.meta.url).pathname;
globalThis.data = await file(here + "urlpatterntestdata.json").json();

// The harness and the vendored WPT runner are plain scripts, not modules, and
// the runner ends with a promise_test that fetches the data over HTTP — that
// tail is dropped, since the data is already loaded above.
const harness = await file(here + "harness.js").text();
const tests = await file(here + "urlpatterntests.js").text();
(0, eval)(`${harness}\n${tests.split("promise_test(")[0]}\nrunTests(data);`);

const failed = results.filter((r) => r.error);
for (const f of failed) console.log(`FAIL ${f.name}\n     ${f.error}`);
console.log(`\n${results.length - failed.length}/${results.length} passing (${failed.length} failing)`);
if (failed.length > 0) throw new Error(`${failed.length} WPT failures`);
