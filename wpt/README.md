# Web Platform Tests — the worker subset

Runs the upstream [Web Platform Tests](https://github.com/web-platform-tests/wpt)
for workers, HTML messaging and structured clone, unmodified, under `esrun`.

```sh
./wpt/fetch.sh                                    # pinned sparse checkout → wpt/upstream
esrun wpt/run.js                                  # every test, both scopes
esrun wpt/run.js -- --mode=worker --verbose       # only inside real workers, listing failures
esrun wpt/run.js -- --filter=webmessaging/        # substring match on the test path
esrun wpt/run.js -- --update-expectations         # re-record the baseline
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

## Scope

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
  subtests        616        573        43         —         —      528       45
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
