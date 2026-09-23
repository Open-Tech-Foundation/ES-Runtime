# Web Platform Tests

Runs the upstream [Web Platform Tests](https://github.com/web-platform-tests/wpt)
unmodified, in two slices that share one pinned checkout:

| Slice | Runner | Under |
| --- | --- | --- |
| workers, HTML messaging, structured clone | `wpt/run.js` | `esrun` |
| `dom`, `custom-elements`, `shadow-dom` | `wpt/dom-run.js` | `esdev test --dom` |

The worker slice is the rest of this document; [the DOM
slice](#the-dom-slice) is at the end.

```sh
./wpt/fetch.sh          # pinned sparse checkout → wpt/upstream

# esrun grants nothing by default; the runner reads the tests, writes
# expectations.json, imports its harness, and spawns real workers.
wpt="esrun --allow-read --allow-write --allow-imports --allow-workers wpt/run.js"

$wpt                                 # every test, both scopes
$wpt -- --mode=worker --verbose      # only inside real workers, listing failures
$wpt -- --filter=webmessaging/       # substring match on the test path
$wpt -- --update-expectations        # re-record the baseline
```

`esrun` claims flags that come before the script name, so the runner's own
arguments go after `--`.

## Why this exists next to `crates/runtime/conformance`

The curated suite states spec behaviour in our own words, which means it can only
contain deviations we already thought of. WPT is written by people who did not
know this runtime existed.

It also runs each test **twice**: once on the agent driving the process, once
inside a real dedicated worker. Nothing in the curated suite runs in a worker at
all, so the worker global scope had no executable coverage before this.

A standard WPT subset is post-1.0 (SPEC §14); this is the beginning of it, scoped
to what workers touch.

## Scope of the worker slice

| Included | Why |
| --- | --- |
| `workers/**/*.any.js`, `*.worker.js` | dedicated workers, module workers, nested workers |
| `webmessaging/**` | `MessagePort`, `MessageChannel`, `BroadcastChannel`, transfer |
| `html/webappapis/structured-clone/**` | the serialization algorithm both of those share |

Excluded, and not counted as failures: `.html`/`.htm` tests (they need a document
and WPT's substituting server), `.sub.js` (server-side substitution), `.window.js`
(window-only by definition), and tests whose only `global=` scopes are
`sharedworker`/`serviceworker`/`shadowrealm` — none of which this runtime has.

## What the numbers mean

```
                total   runnable   skipped   errored   timeout   passed   failed
  files            70         52        18
  runs             77         70         —         2         5
  subtests        618        570        48         —         —      560       10
```

- **total** — everything discovered in the three directories.
- **runnable** — what is a test *of this runtime*: total minus everything
  `scope.js` rules out.
- **skipped** — ruled out by `scope.js`, with a reason per entry. Only for things
  inapplicable **by design** and traceable to a recorded decision — a renderer, a
  document, browser-local storage, classic scripts. Never "not implemented yet".
- **errored** — the file threw before any test could report.
- **timeout** — no result before the deadline; usually a worker that never
  replied, which is a defect, not a slow test.
- **passed / failed** — of the runnable subtests. **`failed` is the number to
  drive to zero**; every one of them is a real deviation or an unimplemented but
  legitimately server-side API.

The pass rate is quoted over *runnable*, so it can reach 100% and a browser-only
test can never flatter or depress it.

## How a test is run

Each test becomes one generated module written next to the original (so relative
URLs inside it still resolve), then deleted:

```
  <prologue: META_TITLE, importScripts shim>
  + resources/testharness.js
  + <collector: add_completion_callback → the runner>
  + every `// META: script=` in order
  + the test itself
      └─ all of it inside (0, eval)(…) in one generated module
           ├─ main mode:   the runner imports it
           └─ worker mode: new Worker(bundle, { permissions: […] })
```

Two deliberate distortions, both forced by this being a module-only runtime:

- **Indirect `eval`.** A WPT test is a classic script — sloppy mode, `var` and
  function declarations on the global, helpers that assign undeclared names
  (`structuredCloneBatteryOfTests = []`). A module body is strict and its scope is
  its own, so tests would fail over how they were loaded rather than what they
  assert. `(0, eval)` is the closest a module-only runtime gets, and is what
  `crates/runtime/conformance/run.js` already does.
- **`importScripts` is a shim** that accepts `/resources/testharness.js` (already
  in the bundle) and throws for anything else. There is no classic-script path to
  implement it against (SPEC §8); the tests that exist only to exercise it are
  out of scope in `scope.js` rather than failing.
- **Root-relative paths are mapped textually.** WPT serves the checkout at `/`,
  so a test may name a helper `"/workers/support/x.js"`. With no server, the
  bundler rewrites those string literals to the checkout path — the same mapping,
  done earlier. Without it, tests fail over how they addressed a file rather than
  what they assert.

`testharness.js` picks its environment by `instanceof DedicatedWorkerGlobalScope`,
so worker mode now selects `DedicatedWorkerTestEnvironment` — which waits to be
told the file has finished adding tests. That is why the bundler appends
`done()` in worker mode, exactly as upstream's own `*.any.worker.js` wrapper
does. Main mode gets `ShellTestEnvironment` and completes on its own.

## Expectations

`expectations.json` records the status of every subtest, per mode. A run compares
against it: a subtest that used to pass and now does not fails the run; one that
starts passing is reported so the record can be updated in the same commit as the
fix. Re-record with `--update-expectations`.

## The 10 subtests that stay failing, and the one timeout

All ten are in `workers/modules/dedicated-worker-import.any.js`, and both of the
reasons are deliberate rather than unfinished:

- **Five are dynamic `import()` inside a worker the test spawned itself**
  (`Dynamic import.`, `Nested dynamic import.`, `Static import and then dynamic
  import.`, `Dynamic import and then static import.`, `eval(import()).`), each
  run in both modes. A worker starts with no capabilities (D48), and a browser's
  `new Worker(url, { type: "module" })` has no way to grant one — so a worker the
  *test* starts holds nothing, and `import()` needs `imports`.

  The worker's *static* graph still loads: its parent resolves it up front, from
  literal specifiers in source the parent already read, with no guest code
  running during instantiation. `import()` computes its specifier at runtime, so
  it is a read-and-execute primitive on the worker's own authority and is gated.
  The refusal now says where the grant is made.

  These are **not** excluded in `scope.js`, by that file's own rule: the tests
  are applicable, and it is a judgment call that they fail — the same treatment
  `data:`/`blob:` worker URLs get. Counting them keeps the number honest.

- **The file's harness status is `ERROR`** because two of its cases are
  cross-origin (`*-remote-origin-*.sub.js`) and import
  `https://{{domains[www1]}}:{{ports[https][0]}}/…`. A `.sub.js` file is a
  template the WPT *server* substitutes; with no server the placeholders survive
  into the specifier. Remote modules are a stated non-goal either way, so these
  can never pass here. (They used to be reported as a missing npm package, since
  an unsubstituted URL fails to parse as one and fell through to the
  `node_modules` walk. Now they say what they are.)

One `TIMEOUT` also remains, and stays on purpose:
`webmessaging/MessageEvent-trusted.any.js:main` builds its worker from a `blob:`
URL and sets no `onerror`, so the failure reaches nothing and the file waits out
its deadline. Its **worker** mode passes, so excluding the file would hide real
coverage — ten seconds is the honest price. The two `message-channels/worker*`
files had the same shape in *both* modes, contributed nothing, and are excluded
in `scope.js`; that took a full run from ~51s to ~11s.

## Runtime bugs this runner found

Building it turned up four, all now fixed and recorded in `CHANGELOG.md`. Listed
because each one shaped the runner, and because the shapes are worth knowing.

1. ~~**`exit()` hangs a module that used top-level `await`**~~ unless it was the
   very last statement — the process parked in `epoll_wait` forever. The runner
   now ends with `exit()`, which is what lets a `--mode=main` sweep terminate at
   all: tests there start workers of their own and never terminate them, and a
   live worker keeps the process alive, correctly.

   ```js
   await null;
   exit(0);
   console.log("unreachable");   // never ran, and the process never exited
   ```

2. ~~**A dynamic `import()` resolves only when the event loop next wakes for some
   other reason.**~~ With a pending timer it was delayed by that timer's *full*
   duration — arming the per-test deadline before the import made every import
   take the whole timeout and every test "time out". The deadline is still armed
   after the import, which is the honest order regardless.

   ```js
   setTimeout(() => {}, 3000);
   await import("./x.js");       // resolved after 3000 ms, not 3 ms
   ```

3. ~~**`write()` resolves before the bytes are on disk, above 64 KiB.**~~ Fixed —
   the provider now flushes before resolving. Every bundle here is over that
   threshold (`testharness.js` is 194 KiB alone), so this runner was where it
   surfaced.

4. ~~**`terminate()` does not terminate a worker's own workers.**~~ Fixed — a
   terminated worker now takes the workers it started with it, and so does one
   that ends by itself. `--mode=worker` exits on its own because of it.

   Tests running on the driver agent still leak workers of their own, which is
   not a defect either — a live worker is a reason for the process to stay up, as
   in Node and Deno. In a browser the page goes away; here the runner's final
   `exit()` does.

## The DOM slice

`wpt/dom-run.js` runs the layout-free JavaScript DOM tests — `dom`,
`custom-elements`, `shadow-dom` — against `esdev test --dom`, one `esdev`
process per file.

```sh
./wpt/fetch.sh                                    # the same pinned checkout
tsr test:dom-wpt                                  # or, directly:
deno run --allow-read --allow-run --allow-write wpt/dom-run.js

# --filter=<substring>   only matching paths
# --update-expectations  re-record dom-expectations.json
# --verbose              one line per file, to stderr, as it goes
# --json=<path>          write the report as well as printing it
# --timeout=<ms>         per file, default 10000
# --esdev=<path>         default target/debug/esdev
# --jobs=<n>             files at once, default up to 8
# --keep                 leave each generated test file in place
# --trace                each subtest's result as it finishes, to stderr
```

Collected: `.any.js`, `.window.js`, and every `.html` page that loads
`testharness.js` (reftests, crash tests and the pages under `resources/` and
`support/` report no subtests and are not). An `.html` page used to be left out
because the runner would have to supply its document, making the result a test
of the runner. It supplies as little as it can: the page's own markup goes
through this DOM's `DOMParser` and is adopted into the document, so what builds
the page is the parser and `adoptNode` under test, not runner code. Its
`<script>` elements stay in the tree, and each classic script then runs in tree
order at global scope; one that throws is reported and the next still runs,
then `load` fires. Leaving out the `.html` pages had hidden entire interfaces:
`CharacterData`'s editing methods and `Text.splitText` were missing, and only
their `.html` tests would have said so.

Three differences from a browser remain. Every element exists before the first
script runs, where a browser parses up to each script. A page script's top
level runs sloppy: an indirect eval of `"use strict"` code keeps its `var`s to
itself, where a classic script makes them global, so the runner prefixes `;`
to make the directive an ordinary expression. Functions inside that declare
`"use strict"` stay strict. And a page the strict
parser refuses is **skipped with the parser's message** rather than counted as a
DOM failure: refusing markup that omits optional tags is D93, not a bug to count
against the DOM. `--keep` leaves the generated file in place to run by hand.

`wpt/chrome-run.js` asks Chrome the same questions. It serves `wpt/upstream`
over local HTTP and loads each test as a real page, `.html` as itself and
`.any.js`/`.window.js` through the wrapper page WPT's server would generate.
`tsr test:dom-wpt-chrome -- --gaps` runs every file with a recorded failure and
lists only the subtests Chrome passes. That list is the work. A subtest Chrome
fails too is upstream running ahead of the browsers.

`dom-scope.js` rules out the rest from the path and from the page's source,
including every script it loads. That covers server substitution, full Web IDL
exposure, tentative APIs, nested browsing contexts (an `<iframe>`,
`contentWindow`, `window.open`), WebDriver automation (`testdriver.js`),
scrolling, CSS animation, legacy APIs and script execution. The report names
the reason per file. Files run in parallel (`--jobs=`, default the machine's
parallelism up to 8) and are accounted in path order.

Each file becomes one generated module beside the original, the same shape the
worker slice uses and for the same reason:

```
  resources/testharness.js
  + <collector: add_completion_callback → the runner>
  + every `// META: script=` in order
  + the test itself
      └─ all of it inside (0, eval)(…), imported by `esdev test --dom --file=`
```

One patch is applied to `testharness.js`: its environment check, so it selects
`ShellTestEnvironment` and reports completion rather than waiting for a page
that does not exist. A patch that stops matching **throws** rather than running
unpatched, since the alternative is reporting the harness's own confusion as DOM
failures. (A second patch used to disable the harness's `AbortController`;
`AbortSignal` now dispatches in its own realm, and removing it changes no
result.)

`--file=` is the per-file *child* of `esdev test`, so the timeout the parent
would have applied is the runner's to apply. Without it a harness that never
completes waits forever; with it the file is reported as `TIMEOUT`. A file that
completes no harness at all exits `0` and prints nothing, and is reported as
`INCOMPLETE`: every task ran and the harness was still waiting on something
that never happened.

`dom-expectations.json` records every subtest's status per file, the same
contract the worker slice works to: a recorded `PASS` that stops passing fails
the run, a subtest that starts passing is reported so the record can be updated
in the commit that fixed it, and `--update-expectations` re-records. The
recorded floor is **97 passing, 45 failing, 35 not run, one harness `ERROR`** —
the failures are real deviations and each one is work, but none of them can get
quietly worse.
