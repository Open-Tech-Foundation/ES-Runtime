// runtime:test — the test API, as a module you import (esdev only).
//
//   import { test, assert, assertEquals, assertThrows } from "runtime:test";
//
//   test("adds", () => assertEquals(add(2, 3), 5));
//
// It used to be five globals, prepended to every test file's own source as a
// single physical line so that the file's line 1 stayed line 1. That worked,
// and it was wrong for three reasons that only an import fixes:
//
//   * **Ambient globals are what this runtime does not do.** Every other piece
//     of host functionality here is a `runtime:` module. A test file was the
//     one place a program was handed names it never asked for.
//   * **Only the entry got them.** The harness was injected into the file being
//     run, so a shared `test-helpers.ts` next to it could not use `assertEquals`
//     — the one place a test suite most wants to share code.
//   * **They had no types.** There was nothing to declare them in, so a `.ts`
//     test file referenced five undeclared names and `tsc --noEmit` failed on
//     a suite that ran perfectly.
//
// The runner keeps the results, not this module: `test()` tells the host a case
// exists and how it ended, and `esdev` prints the summary and decides the exit
// code once the program is done. That is what removes the epilogue that used to
// be appended too — so a test file is now, from the first byte to the last,
// exactly the file the developer wrote.
//
// # One at a time
//
// A test used to *start* where it was written: `test()` called the function and
// returned, so every async case in a file ran at once. It was cheap and it was
// wrong. Two tests that share a database, a temp directory, a port or a module
// global interleave, and the failure is a flake nobody can reproduce; a
// `beforeEach` cannot exist at all, because there is no "before" — the next
// test has already started. The 230 lines of scheduler every suite ended up
// writing to get around it were the evidence.
//
// So registration and execution are separate. `test()` appends to a queue, and
// the queue drains one case at a time, in the order the file wrote them, with
// the lifecycle hooks around each. A test that awaits does hold up the next —
// deliberately: that is what "one at a time" means, and it is what makes shared
// state usable.
//
// The host is told about a case when it is **registered**, not when it starts,
// so a case that never got to run because an earlier one hung is still in the
// report — as a failure that says exactly that.
//
// # Groups
//
// `describe()` is a name and a scope, and the scope is the half that matters: a
// `beforeEach` written inside one belongs to the tests inside it, and a file
// that sets up a database for six of its twenty cases should not be setting it
// up for the other fourteen. Without that, a group is a naming convention, and
// a naming convention is a thing a template string already does.
//
// The body runs **synchronously**, at once, and registers; it is not where
// awaiting belongs. An `async` one is refused rather than half-run, because
// half of it registers before the first `await` and the rest lands after the
// queue has already drained.
//
// # skip and only
//
// A skipped case is **reported as skipped**, not left out. This runner already
// treats a case that never finished as a failure rather than a silence, for the
// reason that decides this too: a green run that quietly ran fewer tests than it
// printed is the worst thing a test runner can do. `only` is the same statement
// from the other side — the cases it did not run are counted and said out loud,
// so a `.only` left in a commit is visible in the tally rather than being a
// suite that passes in a tenth of the time.

const ops = globalThis.__ops;

// What the command line asked of this file: `--test-name-pattern` and
// `--test-skip-pattern`, matched against a test's full name.
const runOptions = JSON.parse(ops.test_options?.() ?? "{}");
const pattern = (flag, source) => {
  if (!source) return null;
  try {
    return new RegExp(source);
  } catch (err) {
    throw new SyntaxError(`${flag}: ${err.message}`);
  }
};
const namePattern = pattern("--test-name-pattern", runOptions.namePattern);
const skipPattern = pattern("--test-skip-pattern", runOptions.skipPattern);
// `--randomize` / `--seed`: the order tests run in, shuffled from a seed so an
// order that failed can be run again. Sibling tests are shuffled among
// themselves inside each group, and groups among theirs — never across a
// group's edge, so a `beforeAll` still wraps exactly its own tests.
const seed = Number.isInteger(runOptions.seed) ? runOptions.seed : null;
const nextRandom = seed === null ? null : mulberry32(seed);

function mulberry32(start) {
  let state = start >>> 0;
  return () => {
    state = (state + 0x6d2b79f5) | 0;
    let t = Math.imul(state ^ (state >>> 15), 1 | state);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

function shuffleQueue() {
  const root = { children: [] };
  const nodes = new Map();
  const node = (scope) => {
    if (scope === null || scope === file) return root;
    let made = nodes.get(scope);
    if (made === undefined) {
      made = { children: [] };
      nodes.set(scope, made);
      node(scope.parent).children.push({ group: made });
    }
    return made;
  };
  for (const entry of queue) node(entry.scope).children.push({ entry });
  const ordered = [];
  const walk = (at) => {
    for (let i = at.children.length - 1; i > 0; i--) {
      const j = Math.floor(nextRandom() * (i + 1));
      [at.children[i], at.children[j]] = [at.children[j], at.children[i]];
    }
    for (const child of at.children) {
      if (child.entry) ordered.push(child.entry);
      else walk(child.group);
    }
  };
  walk(root);
  queue.splice(0, queue.length, ...ordered);
}

// `--repeats`: how many more times every test runs, unless it says otherwise.
const defaultRepeats = Number.isInteger(runOptions.repeats) ? runOptions.repeats : 0;

// `--bail`: how many tests may fail in this run before the rest are not run.
const bailAt = Number.isInteger(runOptions.bail) ? runOptions.bail : null;
let failedTests = 0;

const selected = (title) =>
  (namePattern === null || namePattern.test(title)) && (skipPattern === null || !skipPattern.test(title));
let activeCase = null;
// How many snapshots of each name the running attempt has taken. Counted per
// name, so a named snapshot keeps its key when another is added before it.
let snapshotCounts = new Map();
// What the running attempt of a case has asked for and done: its assertion
// count, what `expect.assertions` wants of it, its soft failures, and the
// callbacks it registered for when it ends. `null` between cases.
let attempt = null;

// Cases waiting to run, in the order they were written.
const queue = [];

// A group: a name, the hooks that belong to it, and what it is still waiting
// for. The file itself is one, with no name — which is what makes a hook
// written outside any `describe` the outermost scope rather than a special
// case.
function group(name, parent) {
  return {
    name,
    parent,
    // Each in registration order. Several of the same kind are allowed and all
    // of them run: a helper module and the test file both have a right to a
    // `beforeEach`, and the one that loaded second is not the only one.
    hooks: { beforeAll: [], afterAll: [], beforeEach: [], afterEach: [] },
    // Cases registered in it or in a group inside it, still to finish. What
    // decides when its `afterAll` runs.
    left: 0,
    opened: false,
    closed: false,
    // What its `beforeAll` threw, if it did: every case under it then fails
    // with that rather than running against a fixture that was never built.
    failure: null,
    skip: false,
    only: false,
  };
}

// The file's own scope, and the one being registered into right now.
const file = group("", null);
let current = file;
// Every group made, in the order they were made — so what is left open at the
// end is closed innermost first.
const groups = [file];

// Whether a drain is already scheduled or running, so registering ten tests in
// a row schedules one.
let draining = false;
// Whether anything anywhere asked to be the only thing that runs. Sticky: a
// `.only` registered after a drain has begun still speaks for the cases behind
// it, and un-deciding it later would run tests the file said not to.
let exclusive = false;

// The runner's own task boundary, captured before any test can swap
// `setTimeout` for a frozen clock: a file that stops time must not stop the
// runner with it.
const realSetTimeout = globalThis.setTimeout;
const realClearTimeout = globalThis.clearTimeout;
// The drain is scheduled on a microtask; a clock that fakes `queueMicrotask`
// must not hold it back.
const realQueueMicrotask = globalThis.queueMicrotask;
// The real clock, for the same reason: `expect.poll` and `waitFor` measure how
// long they have waited, and a frozen `Date` would never let them stop.
const realNow = Date.now;

// An unhandled rejection is the running test's failure, not the file's death.
// Without this the process is torn down where the rejection surfaced, and a
// suite of a hundred passing tests reports nothing at all — the one thing this
// runner is most careful never to do. One that surfaces between cases has no
// test to belong to, so it is reported as a case of its own.
let pendingRejection = null;
const strayRejections = [];

function claimRejection(event) {
  // A suite that listens for rejections itself and claims them — which is how
  // a framework's own error-handling tests are written — has already said the
  // rejection was expected. The runner is the handler of last resort.
  if (event.defaultPrevented) return;
  // Only while the run owns the process. After it, an unclaimed rejection is
  // the runtime's to report, and swallowing it would hide a real error.
  if (!draining) return;
  event.preventDefault();
  if (activeCase !== null) pendingRejection ??= event.reason;
  else strayRejections.push(event.reason);
}

// Re-armed before each case so it stays *last*: listeners run in registration
// order, and the runner's has to see what a suite's own listener did with the
// event before deciding it went unhandled.
function armRejectionListener() {
  if (typeof globalThis.addEventListener !== "function") return;
  globalThis.removeEventListener("unhandledrejection", claimRejection);
  globalThis.addEventListener("unhandledrejection", claimRejection);
}

armRejectionListener();

// The detail a failure is reported with: the stack when there is one, because a
// failure is only actionable if it names the line that failed. V8 starts a stack
// with the error itself; SpiderMonkey and JavaScriptCore start at the first
// frame, which in a browser run would report where a test failed and never what
// it expected — so the error is put back in front when the stack left it out.
const detail = (err) => {
  if (!err?.stack) return String(err);
  const stack = String(err.stack);
  let head;
  try {
    head = String(err);
  } catch {
    return stack;
  }
  return stack.startsWith(head) ? stack : `${head}\n${stack}`;
};

// The name a case is reported under: its groups, outermost first, then its own.
function label(scope, name) {
  const parts = [name];
  for (let at = scope; at && at.parent; at = at.parent) parts.unshift(at.name);
  return parts.join(" > ");
}

// Every hook of a kind that applies to a case, outermost group first — which is
// the order a `beforeEach` has to run in, and the reverse of an `afterEach`.
function around(scope, kind) {
  const out = [];
  for (let at = scope; at; at = at.parent) out.unshift(...at.hooks[kind]);
  return out;
}

// The `beforeAll` failure a case inherits: its own group's, or the nearest one
// outside it.
function broken(scope) {
  for (let at = scope; at; at = at.parent) if (at.failure !== null) return at.failure;
  return null;
}

function hook(kind) {
  return function register(fn) {
    if (typeof fn !== "function") {
      throw new TypeError(`${kind}(): needs a function to run`);
    }
    // The group being registered into, not the file: a `beforeEach` inside a
    // `describe` is that group's, and running it for the file's other tests is
    // the thing having groups at all is meant to stop.
    current.hooks[kind].push(fn);
  };
}

// Runs once before the first test **of its group**, and once after the last.
//
// At the top level of a file that is once per file. "After the last" is decided
// by the group having no cases left, since a file does not announce that it has
// finished registering — so a test registered after its group has drained runs
// after that group's `afterAll`, which is the only honest answer available
// without a declaration this API does not have.
const beforeAll = hook("beforeAll");
const afterAll = hook("afterAll");
// Runs around every test in scope, including one that fails. `afterEach` is
// cleanup, so it runs whatever happened — a `beforeEach` that threw included.
const beforeEach = hook("beforeEach");
const afterEach = hook("afterEach");

// A group of tests. The body registers and returns; it does not await.
function describe(name, body, mode) {
  if (typeof body !== "function") {
    throw new TypeError(
      `describe(${JSON.stringify(String(name))}): needs a function that registers the tests`,
    );
  }
  const made = group(String(name), current);
  // A group that is skipped skips everything in it, and one marked `only`
  // makes every case in it an `only` — which is what makes `describe.only`
  // mean the group rather than nothing.
  made.skip = mode === "skip" || current.skip;
  made.only = mode === "only" || current.only;
  if (made.only) exclusive = true;
  groups.push(made);
  const outer = current;
  current = made;
  try {
    const returned = body();
    if (returned !== null && typeof returned?.then === "function") {
      throw new TypeError(
        `describe(${JSON.stringify(String(name))}): the body registers tests and returns — ` +
          `it cannot be async, because only the part before its first await would register ` +
          `in time. Await inside a test, or in beforeAll.`,
      );
    }
  } finally {
    current = outer;
  }
}

// Queues a case. It runs when the ones before it have finished.
//
// The id comes back now, at registration, and that is what keeps the report
// complete: a case that never got to start because an earlier one never settled
// is a case the host already knows about, and it is reported as a failure
// rather than silently missing from a green run.
function enqueue(name, fn, mode, options) {
  const skip = mode === "skip" || current.skip;
  if (!skip && typeof fn !== "function") {
    throw new TypeError(`test(${JSON.stringify(String(name))}): needs a function to run`);
  }
  const scope = current;
  const title = label(scope, String(name));
  // A listing names what a filter selected, and nothing else.
  if (runOptions.list === true && !selected(title)) return;
  const id = ops.test_registered(title);
  if (skip) {
    // Reported now and never queued: nothing about it runs, its group's
    // `beforeAll` included.
    ops.test_skipped(id, "");
    return;
  }
  // Left out by a name filter: counted, and said so, like a `.only`'s others.
  if (!selected(title)) {
    ops.test_skipped(id, "filter");
    return;
  }
  // `--list`: named, and never queued — no test and no hook runs.
  if (runOptions.list === true) {
    ops.test_skipped(id, "list");
    return;
  }
  const only = mode === "only" || scope.only;
  if (only) exclusive = true;
  for (let at = scope; at; at = at.parent) at.left += 1;
  queue.push({ id, fn, scope, only, options });
  schedule();
}

// What a case may say about itself: `{ timeout, retry }`, or a bare number of
// milliseconds. Written after the body or before it — `test(name, fn, 5000)`
// and `test(name, { retry: 2 }, fn)` are both everyday spellings, and a runner
// that took one would read the other as a test with no body.
function caseArgs(name, a, b) {
  if (typeof a !== "function" && typeof b === "function") return [b, optionsOf(name, a)];
  return [a, optionsOf(name, b)];
}

function optionsOf(name, value) {
  const where = `test(${JSON.stringify(String(name))})`;
  if (value === undefined) return {};
  if (typeof value === "number") value = { timeout: value };
  if (value === null || typeof value !== "object") {
    throw new TypeError(`${where}: options are { timeout, retry }, or a number of milliseconds`);
  }
  const { timeout, retry, repeats } = value;
  if (timeout !== undefined && !(Number.isFinite(timeout) && timeout > 0)) {
    throw new TypeError(`${where}: timeout is a number of milliseconds above zero`);
  }
  if (retry !== undefined && !(Number.isInteger(retry) && retry >= 0)) {
    throw new TypeError(`${where}: retry is how many more times to try, a whole number`);
  }
  if (repeats !== undefined && !(Number.isInteger(repeats) && repeats >= 0)) {
    throw new TypeError(`${where}: repeats is how many more times to run it, a whole number`);
  }
  return { timeout, retry, repeats };
}

// Registers a test. It runs when the ones before it have finished.
function test(name, a, b) {
  const [fn, options] = caseArgs(name, a, b);
  enqueue(name, fn, undefined, options);
}

// `test.skip(...)` and `test.only(...)`, and the same pair on `describe`.
test.skip = (name, a, b) => {
  const [fn, options] = caseArgs(name, a, b);
  enqueue(name, fn, "skip", options);
};
test.only = (name, a, b) => {
  const [fn, options] = caseArgs(name, a, b);
  enqueue(name, fn, "only", options);
};

// A case that is known to fail, and passes for as long as it does. The day it
// starts passing it fails, saying so — which is the point: a fixed bug whose
// test is still marked as broken is a regression test nobody is running.
test.fails = (name, a, b) => {
  const [fn, options] = caseArgs(name, a, b);
  enqueue(name, fn, undefined, { ...options, fails: true });
};
describe.skip = (name, body) => describe(name, body, "skip");
describe.only = (name, body) => describe(name, body, "only");

// A case with a name and no body: work that is planned and not written.
//
// **Counted as skipped, and never silently absent.** The whole runner is
// arranged so a report says what did not run, and a to-do that vanished from
// the tally would be the one kind of missing case nobody notices.
test.todo = (name, fn) => enqueue(name, fn ?? (() => {}), "skip", {});
describe.todo = (name, body) => describe(name, body ?? (() => {}), "skip");

// `test.skipIf(cond)(...)` / `test.runIf(cond)(...)` — a case that depends on
// where it is running. A suite that needs a Postgres to be up has to say so
// somehow, and the alternative is an `if` around the registration, which
// removes the case from the report entirely rather than reporting it skipped.
test.skipIf = (condition) => (condition ? test.skip : test);
test.runIf = (condition) => (condition ? test : test.skip);
describe.skipIf = (condition) => (condition ? describe.skip : describe);
describe.runIf = (condition) => (condition ? describe : describe.skip);

/// One case per row of `table`, named by substituting the row into `name`.
///
/// `%s`/`%d`/`%i`/`%f`/`%j`/`%o` take the next value positionally and `%#` is
/// the row's index, as the ecosystem spells them; `$key` takes a named property
/// when the row is an object. A row that is an array is spread into the body's
/// arguments, so `(a, b, want)` reads like the table's header.
///
/// **The name has to differ per row.** A table whose rows all produce one name
/// is a report where six cases share an identity and a failure names none of
/// them — so an index is appended when the substitution left the name unchanged.
function each(register) {
  return (table) => {
    if (!Array.isArray(table)) {
      throw new TypeError("each(...) needs an array of rows");
    }
    return (name, fn, ...rest) => {
      table.forEach((row, index) => {
        const args = Array.isArray(row) ? row : [row];
        let title = format(String(name), args, index);
        if (title === String(name) && table.length > 1) title = `${title} [${index}]`;
        register(title, () => fn(...args), ...rest);
      });
    };
  };
}

/// The substitution `each` performs on a row's name.
function format(name, args, index) {
  let next = 0;
  let out = name.replace(/%[sdifjo#%]/g, (token) => {
    if (token === "%%") return "%";
    if (token === "%#") return String(index);
    const value = args[next++];
    switch (token) {
      case "%s":
        return typeof value === "string" ? value : show(value);
      case "%d":
      case "%i":
        return String(Number.parseInt(value, 10));
      case "%f":
        return String(Number(value));
      default:
        return show(value);
    }
  });
  // `$key`, for a table of objects — the spelling that makes a row readable.
  const first = args[0];
  if (first !== null && typeof first === "object" && !Array.isArray(first)) {
    out = out.replace(/\$([A-Za-z_$][\w$]*)/g, (whole, key) =>
      key in first ? show(first[key]).replace(/^"|"$/g, "") : whole,
    );
  }
  return out;
}

test.each = each(test);
test.skip.each = each(test.skip);
test.only.each = each(test.only);
test.todo.each = each(test.todo);
test.fails.each = each(test.fails);
describe.each = each(describe);
describe.skip.each = each(describe.skip);
describe.only.each = each(describe.only);

// `it` and `suite` — the same functions under the names the rest of the
// ecosystem writes. Aliases, not variants: two implementations of one thing is
// how they end up disagreeing about `.only`.
const it = test;
const suite = describe;

// Used only by esdev's generated unisolated entry so registrations retain the
// test file they came from. Test authors never need to call it.
const __setTestFile = (file) => ops.test_set_file(String(file));

function schedule() {
  if (draining) return;
  draining = true;
  // A microtask, not a call: the rest of the file is still registering, and a
  // drain that started on the first `test()` would run case one before case two
  // existed. By the time microtasks run, the module body is done.
  Reflect.apply(realQueueMicrotask, globalThis, [
    () => {
      drain();
    },
  ]);
}

async function drain() {
  try {
    if (nextRandom !== null) shuffleQueue();
    while (queue.length > 0) {
      const next = queue.shift();
      // Something asked to be the only thing that runs, and this is not it.
      // Decided here rather than at registration, because whether a case is
      // the exception is not known until the file has finished registering.
      if (exclusive && !next.only) {
        ops.test_skipped(next.id, "only");
        await settled(next.scope);
        continue;
      }
      // The failure limit was reached: what is left is counted, not run.
      if (bailAt !== null && failedTests >= bailAt) {
        ops.test_skipped(next.id, "bail");
        await settled(next.scope);
        continue;
      }
      await runCase(next);
    }
    // Whatever is still open — a group whose last case has run leaves through
    // `settled` below, so this is the file's own scope, and any group a case
    // registered into after its own drain.
    for (const scope of [...groups].reverse()) await close(scope);
    for (const reason of strayRejections.splice(0)) {
      ops.test_finished(ops.test_registered("unhandled rejection"), false, detail(reason));
    }
  } finally {
    draining = false;
    // The queue is empty and every group closed. A process run learns this by
    // reaching quiescence and does not listen; a page never goes quiet, so a
    // browser run is told instead.
    ops.test_drained?.();
  }
}

// Runs a group's `beforeAll`, and its enclosing groups' first. Once each.
async function open(scope) {
  if (scope.parent) await open(scope.parent);
  if (scope.opened) return;
  scope.opened = true;
  // An outer `beforeAll` failed, so this one does not run: it would be setting
  // up on top of something that was never built.
  if (broken(scope.parent) !== null) return;
  try {
    for (const fn of scope.hooks.beforeAll) await fn();
  } catch (err) {
    scope.failure = err;
  }
}

// Runs a group's `afterAll`, if its `beforeAll` ran.
async function close(scope) {
  if (!scope.opened || scope.closed) return;
  scope.closed = true;
  for (const fn of scope.hooks.afterAll) {
    try {
      await fn();
    } catch (err) {
      // Nothing is left to fail, so it is reported as a case of its own — a
      // teardown that threw is a broken suite, not a footnote.
      ops.test_finished(ops.test_registered(label(scope, "afterAll")), false, detail(err));
    }
  }
}

// One case is done. A group with nothing left is finished with, innermost
// first — so an inner `afterAll` runs before the outer one that set up what it
// is tearing down.
async function settled(scope) {
  for (let at = scope; at; at = at.parent) {
    at.left -= 1;
    if (at.left === 0) await close(at);
  }
}

async function runCase({ id, fn, scope, options }) {
  ops.test_running(id);
  await open(scope);
  const failed = broken(scope);
  if (failed !== null) {
    ops.test_finished(id, false, `beforeAll failed, so this test never ran\n${detail(failed)}`);
    await settled(scope);
    return;
  }
  // `repeats` more runs after the first, as Vitest and Bun count them — to
  // find a test that passes only sometimes. Each run must pass; the first that
  // fails ends it. Within a run, `retry` more attempts after the first, each a
  // whole run of the case — its `beforeEach`, its body and its `afterEach` —
  // so a retry starts from the same state the first attempt did. Only the last
  // attempt is reported.
  const runs = 1 + (options.repeats ?? defaultRepeats);
  const attempts = 1 + (options.retry ?? 0);
  let failure = null;
  let failedRun = 0;
  let started = false;
  for (let run = 1; run <= runs && failure === null; run++) {
    for (let tried = 1; tried <= attempts; tried++) {
      // Said again for each run and retry: the host counts an attempt's
      // snapshot results, and only the last attempt's are the case's.
      if (started) ops.test_running(id);
      started = true;
      failure = await runAttempt(id, fn, scope, options);
      if (options.fails) {
        failure =
          failure === null
            ? new Error("expected this test to fail, and it passed — remove test.fails if it is fixed")
            : null;
      }
      if (failure === null) break;
    }
    if (failure !== null) failedRun = run;
  }
  let reported = failure === null ? "" : detail(failure);
  if (failure !== null && attempts > 1) reported = `failed ${attempts} attempts; the last:\n${reported}`;
  if (failure !== null && runs > 1) reported = `failed on run ${failedRun} of ${runs}\n${reported}`;
  ops.test_finished(id, failure === null, reported);
  if (failure !== null) failedTests += 1;
  await settled(scope);
}

// One attempt at a case, and what it failed with, or `null`.
async function runAttempt(id, fn, scope, options) {
  let failure = null;
  const state = { assertions: 0, expected: null, atLeastOne: false, soft: [], finished: [], failed: [] };
  attempt = state;
  try {
    activeCase = id;
    snapshotCounts = new Map();
    pendingRejection = null;
    armRejectionListener();
    for (const before of around(scope, "beforeEach")) await before();
    await (options.timeout === undefined ? fn() : within(fn, options.timeout));
    if (state.expected !== null && state.assertions !== state.expected) {
      throw new Error(
        `expected ${state.expected} assertion${state.expected === 1 ? "" : "s"}, and ${state.assertions} ran`,
      );
    }
    if (state.atLeastOne && state.assertions === 0) {
      throw new Error("expected at least one assertion, and none ran");
    }
  } catch (err) {
    failure = err;
  }
  // Soft failures are the case's failures too, reported together — with a
  // hard one that ended the case, if there was one.
  if (state.soft.length > 0) failure = gathered(failure === null ? state.soft : [...state.soft, failure]);
  // Whatever the case left queued runs before its cleanup does. Crossing a task
  // boundary is the only way to know the microtask queue is empty, and a
  // teardown that runs while the case's own promise chain is still settling is
  // a race the case loses — a suite written against a browser runner expects
  // its trailing `.then()` to see the DOM it rendered, not a torn-down one.
  await new Promise((resolve) => Reflect.apply(realSetTimeout, globalThis, [resolve, 0]));
  for (const after of around(scope, "afterEach").reverse()) {
    try {
      await after();
    } catch (err) {
      // A cleanup that threw fails the case, unless the case had already
      // failed — the first failure is the one that explains the rest.
      failure ??= err;
    }
  }
  // A promise this case left rejected fails it, like a thrown error — unless
  // something already did, since the first failure explains the rest.
  if (failure === null && pendingRejection !== null) failure = pendingRejection;
  pendingRejection = null;
  // The case's own cleanup, after the hooks and newest first, as a stack of
  // things to undo is unwound.
  if (failure !== null) {
    for (const callback of state.failed) {
      try {
        await callback(failure);
      } catch {
        // The case has already failed, and with a better reason.
      }
    }
  }
  for (const callback of state.finished.reverse()) {
    try {
      await callback();
    } catch (err) {
      failure ??= err;
    }
  }
  activeCase = null;
  attempt = null;
  return failure;
}

// A case's body, failed if it has not settled within `ms`. The body is not
// stopped — nothing in JavaScript can stop it — so it may still be running
// while the next case starts; the timeout says which case to look at.
function within(fn, ms) {
  return new Promise((resolve, reject) => {
    const timer = Reflect.apply(realSetTimeout, globalThis, [
      () => reject(new Error(`the test did not finish within ${ms}ms`)),
      ms,
    ]);
    Promise.resolve()
      .then(fn)
      .then(resolve, reject)
      .finally(() => Reflect.apply(realClearTimeout, globalThis, [timer]));
  });
}

// Several failures as one: each one's message, numbered, then the first one's
// stack for where to start looking.
function gathered(failures) {
  if (failures.length === 1) return failures[0];
  const lines = failures.map((err, index) => `  ${index + 1}. ${showError(err)}`);
  return restated(`${failures.length} assertions failed:\n${lines.join("\n")}`, failures[0]);
}

// A new error saying `message`, at the frames `from` was thrown at. Its stack
// is written with the message rather than edited afterwards: an engine that
// captured the stack text when the error was made keeps the old message in it.
function restated(message, from) {
  const error = new Error(message);
  if (from !== null && typeof from === "object") {
    if (typeof from.name === "string") error.name = from.name;
    if (typeof from.stack === "string") {
      // V8 frames read `    at …`, SpiderMonkey and JavaScriptCore `name@url`.
      const frames = from.stack.split("\n").filter((line) => /^\s+at |@\S+:\d+:\d+$/.test(line));
      error.stack = [String(error), ...frames].join("\n");
    }
  }
  return error;
}

// `onTestFinished(fn)` — cleanup that belongs to the running test, registered
// where the thing it cleans up was made rather than in a hook far from it.
function onTestFinished(fn) {
  duringTest("onTestFinished", fn).finished.push(fn);
}

// `onTestFailed(fn)` — runs only if the running test failed, and is given why:
// the place to dump the state a failure needs explaining.
function onTestFailed(fn) {
  duringTest("onTestFailed", fn).failed.push(fn);
}

function duringTest(name, fn) {
  if (typeof fn !== "function") throw new TypeError(`${name}(): needs a function to run`);
  if (attempt === null) throw new Error(`${name}() must be called inside a test or its beforeEach`);
  return attempt;
}

// One assertion made by the running case, for `expect.assertions`.
function counted() {
  if (attempt !== null) attempt.assertions += 1;
}

function assert(condition, message) {
  counted();
  if (!condition) throw new Error(message || "assertion failed");
}

// Keys that carry a value. `{ a: 1, b: undefined }` and `{ a: 1 }` are the same
// object to anyone reading them, and an equality test that disagreed would fail
// on the difference between a field left out and a field set to nothing.
const definedKeys = (o) => Object.keys(o).filter((k) => o[k] !== undefined);

const sameBytes = (a, b) => {
  if (a.length !== b.length) return false;
  for (let i = 0; i < a.length; i++) if (a[i] !== b[i]) return false;
  return true;
};

// Structural equality, deliberately not `JSON.stringify` on both sides.
//
// That is how this started, and it was wrong in a way that mattered on this
// runtime specifically: `JSON.stringify` *throws* on a BigInt, so the one
// assertion an int64 test most needs could not be written; a Uint8Array
// stringified to `{"0":1,"1":2}` instead of comparing as bytes; and object key
// order decided the result, which no equality test wants.
function equal(a, b, seen, strict = false) {
  // An asymmetric matcher stands where a value would: `expect.any(Number)`
  // inside an expected object is a *predicate*, not something to compare with.
  // Checked before anything else so it works at any depth — which is the only
  // reason to have them, since a top-level one could be its own assertion.
  if (isMatcher(b)) return b.matches(a);
  if (isMatcher(a)) return a.matches(b);
  // Then the testers `expect.addEqualityTesters` added: the first to answer
  // true or false decides; `undefined` passes the pair on.
  for (const tester of testers) {
    const verdict = tester.call(testerContext, a, b, testers);
    if (verdict !== undefined) return Boolean(verdict);
  }
  if (Object.is(a, b)) return true;
  if (typeof a !== typeof b) return false;
  if (a === null || b === null || typeof a !== "object") return false;

  const tag = Object.prototype.toString.call(a);
  if (tag !== Object.prototype.toString.call(b)) return false;
  // Strictly, a class instance is not a plain object with the same fields.
  if (strict && Object.getPrototypeOf(a) !== Object.getPrototypeOf(b)) return false;

  // Compared by what identifies them, not by their fields.
  if (a instanceof Date) return a.getTime() === b.getTime();
  if (a instanceof RegExp) return a.source === b.source && a.flags === b.flags;
  if (a instanceof Error) return a.name === b.name && a.message === b.message;

  // A pair already being compared is assumed equal: that is what makes a cyclic
  // structure terminate instead of blowing the stack.
  for (const pair of seen) if (pair[0] === a && pair[1] === b) return true;
  seen.push([a, b]);

  if (a instanceof ArrayBuffer) return sameBytes(new Uint8Array(a), new Uint8Array(b));
  if (ArrayBuffer.isView(a)) {
    return sameBytes(
      new Uint8Array(a.buffer, a.byteOffset, a.byteLength),
      new Uint8Array(b.buffer, b.byteOffset, b.byteLength),
    );
  }

  if (a instanceof Map) {
    if (a.size !== b.size) return false;
    for (const [k, v] of a) {
      // The fast path: an identical key. Otherwise every entry has to be tried,
      // because two structurally equal keys are not the same object.
      if (b.has(k)) {
        if (!equal(v, b.get(k), seen, strict)) return false;
        continue;
      }
      let found = false;
      for (const [k2, v2] of b) {
        if (equal(k, k2, seen, strict) && equal(v, v2, seen, strict)) {
          found = true;
          break;
        }
      }
      if (!found) return false;
    }
    return true;
  }

  if (a instanceof Set) {
    if (a.size !== b.size) return false;
    for (const v of a) {
      if (b.has(v)) continue;
      let found = false;
      for (const v2 of b) {
        if (equal(v, v2, seen, strict)) {
          found = true;
          break;
        }
      }
      if (!found) return false;
    }
    return true;
  }

  if (Array.isArray(a)) {
    if (a.length !== b.length) return false;
    for (let i = 0; i < a.length; i++) {
      // Strictly, a hole is not an `undefined`.
      if (strict && i in a !== i in b) return false;
      if (!equal(a[i], b[i], seen, strict)) return false;
    }
    return true;
  }

  // Strictly, a key set to `undefined` is not a key left out.
  const ka = strict ? Object.keys(a) : definedKeys(a);
  const kb = strict ? Object.keys(b) : definedKeys(b);
  if (ka.length !== kb.length) return false;
  for (const k of ka) {
    if (!Object.prototype.hasOwnProperty.call(b, k)) return false;
    if (!equal(a[k], b[k], seen, strict)) return false;
  }
  return true;
}

// A value, for a failure message. Everything `JSON.stringify` refuses or
// mangles is handled first, because those are exactly the values a failing
// assertion is most often about.
function show(v) {
  if (typeof v === "bigint") return `${v}n`;
  if (v !== null && typeof v === "object" && typeof v.nodeType === "number" && typeof v.nodeName === "string") {
    return describeNode(v);
  }
  if (v === undefined || typeof v === "symbol" || typeof v === "function") return String(v);
  try {
    const s = JSON.stringify(v, (_key, x) => {
      if (typeof x === "bigint") return `${x}n`;
      if (ArrayBuffer.isView(x) && !(x instanceof DataView)) return Array.from(x);
      if (x instanceof Map) return Array.from(x);
      if (x instanceof Set) return Array.from(x);
      return x;
    });
    return s === undefined ? String(v) : s;
  } catch {
    return String(v);
  }
}

// Snapshots are a deliberately small, tagged tree rather than JavaScript text.
// The tags keep `undefined`, bigint and non-finite numbers distinct, while
// sorting own string keys makes equivalent plain data produce the same bytes.
// A reference gets an id before its contents so shared objects and cycles stay
// visible instead of being silently duplicated or rejected.
/// Testers `expect.addEqualityTesters` added, in the order they were added.
const testers = [];
/// `this` in a tester: `this.equals(a, b)` compares deeply, testers and all.
const testerContext = Object.freeze({ equals: (a, b) => equal(a, b, []) });

/// Serializers `expect.addSnapshotSerializer` added, newest first.
const serializers = [];

/// The formatting options a serializer's `serialize` is given, as
/// pretty-format names them. Snapshots here are uncoloured and indented by two.
const plain = { open: "", close: "" };
const SERIALIZER_CONFIG = Object.freeze({
  indent: "  ",
  min: false,
  maxDepth: Infinity,
  maxWidth: Infinity,
  spacingInner: "\n",
  spacingOuter: "\n",
  escapeRegex: false,
  escapeString: true,
  printBasicPrototype: true,
  printFunctionName: true,
  callToJSON: true,
  plugins: [],
  colors: { comment: plain, content: plain, prop: plain, tag: plain, value: plain },
});

function snapshotValue(value) {
  const seen = new Set();
  const pad = (depth) => "  ".repeat(depth);
  const own = (object, depth) => Object.keys(object).sort().map((key) => {
    const descriptor = Object.getOwnPropertyDescriptor(object, key);
    if (!descriptor || !("value" in descriptor)) throw new TypeError("snapshots do not invoke getters");
    return `${pad(depth)}${JSON.stringify(key)}: ${print(descriptor.value, depth)},`;
  });
  const block = (open, lines, close, depth) => lines.length === 0 ? `${open}${close}` : `${open}\n${lines.join("\n")}\n${pad(depth - 1)}${close}`;
  const print = (v, depth) => {
    // A serializer first, as in Jest and Vitest: the newest whose `test` takes
    // the value. Its children print at the depth its indentation says.
    for (const plugin of serializers) {
      if (!plugin.test(v)) continue;
      const printer = (child, _config, indentation) =>
        print(child, typeof indentation === "string" ? Math.floor(indentation.length / 2) : depth + 1);
      if (typeof plugin.serialize === "function") {
        return String(plugin.serialize(v, SERIALIZER_CONFIG, pad(depth), depth, [], printer));
      }
      const indent = (text) => text.split("\n").map((line) => `  ${line}`).join("\n");
      return String(plugin.print(v, (child) => print(child, depth), indent, SERIALIZER_CONFIG, SERIALIZER_CONFIG.colors));
    }
    if (v === null) return "null";
    if (v === undefined) return "undefined";
    if (typeof v === "string") return JSON.stringify(v);
    if (typeof v === "boolean") return String(v);
    if (typeof v === "bigint") return `${v}n`;
    if (typeof v === "number") return Number.isNaN(v) ? "NaN" : v === Infinity ? "Infinity" : v === -Infinity ? "-Infinity" : Object.is(v, -0) ? "-0" : String(v);
    if (typeof v === "function" || typeof v === "symbol") throw new TypeError("snapshots do not support functions or symbols");
    if (v && v[SNAPSHOT_MATCHER]) return v.label;
    if (seen.has(v)) return "[Circular]";
    seen.add(v);
    if (Array.isArray(v)) return block("[", v.map((item) => `${pad(depth + 1)}${print(item, depth + 1)},`), "]", depth + 1);
    if (v instanceof Date) return `Date(${JSON.stringify(v.toISOString())})`;
    if (v instanceof RegExp) return v.toString();
    // Sorting on the printed form, rather than String(value), preserves the
    // assertion library's order-insensitive Map/Set semantics for objects too.
    if (v instanceof Map) return block("Map {", Array.from(v).sort(([a, av], [b, bv]) => {
      const left = `${snapshotValue(a)} => ${snapshotValue(av)}`;
      const right = `${snapshotValue(b)} => ${snapshotValue(bv)}`;
      return left.localeCompare(right);
    }).map(([k, item]) => `${pad(depth + 1)}${print(k, depth + 1)} => ${print(item, depth + 1)},`), "}", depth + 1);
    if (v instanceof Set) return block("Set {", Array.from(v).sort((a, b) => snapshotValue(a).localeCompare(snapshotValue(b))).map((item) => `${pad(depth + 1)}${print(item, depth + 1)},`), "}", depth + 1);
    if (v instanceof ArrayBuffer || ArrayBuffer.isView(v)) {
      const bytes = v instanceof ArrayBuffer ? new Uint8Array(v) : new Uint8Array(v.buffer, v.byteOffset, v.byteLength);
      return `${v instanceof ArrayBuffer ? "ArrayBuffer" : v.constructor.name} [${Array.from(bytes).join(", ")}]`;
    }
    if (v instanceof Error) {
      // Error#cause is normally non-enumerable, but it is diagnostic state
      // rather than an implementation detail. Preserve it alongside ordinary
      // own fields such as `code` without invoking a getter.
      const properties = Object.fromEntries(Object.keys(v).map((key) => [key, v[key]]));
      if (Object.hasOwn(v, "cause")) properties.cause = v.cause;
      return `${v.name}(${JSON.stringify(v.message)})${Object.keys(properties).length ? ` ${block("{", own(properties, depth + 1), "}", depth + 1)}` : ""}`;
    }
    if (v instanceof Promise || v instanceof WeakMap || v instanceof WeakSet) throw new TypeError("snapshots do not support asynchronous or weak collections");
    const prototype = Object.getPrototypeOf(v);
    if (prototype !== Object.prototype && prototype !== null) {
      throw new TypeError(
        `snapshots support plain objects, not class or host instances (${v?.constructor?.name ?? "this one"}); expect.addSnapshotSerializer can print one`,
      );
    }
    return block("{", own(v, depth + 1), "}", depth + 1);
  };
  return print(value, 0);
}

function maskSnapshot(value, pattern) {
  if (isMatcher(pattern)) {
    if (!pattern.matches(value)) throw new Error(`snapshot property did not match ${pattern.label}`);
    return Object.freeze({ [SNAPSHOT_MATCHER]: true, label: pattern.label });
  }
  if (pattern === null || typeof pattern !== "object" || value === null || typeof value !== "object") {
    return value;
  }
  if (Array.isArray(value)) {
    return value.map((item, index) => index in pattern ? maskSnapshot(item, pattern[index]) : item);
  }
  const copy = { ...value };
  for (const key of Object.keys(pattern)) {
    if (!Object.prototype.hasOwnProperty.call(value, key)) {
      throw new Error(`snapshot property ${JSON.stringify(key)} is missing`);
    }
    copy[key] = maskSnapshot(value[key], pattern[key]);
  }
  return copy;
}

function snapshot(actual, nameOrMatchers, kind = "value") {
  if (activeCase === null) throw new Error("toMatchSnapshot must run inside a test");
  let name = nameOrMatchers;
  if (nameOrMatchers !== undefined && typeof nameOrMatchers !== "string") {
    if (nameOrMatchers === null || typeof nameOrMatchers !== "object") throw new TypeError("toMatchSnapshot(nameOrMatchers) needs a string name or object matchers");
    if (!matchesObject(actual, nameOrMatchers, [])) throw new Error("snapshot value did not satisfy its property matchers");
    actual = maskSnapshot(actual, nameOrMatchers);
    name = undefined;
  }
  const base = name === undefined ? "snapshot" : String(name);
  const count = (snapshotCounts.get(base) ?? 0) + 1;
  snapshotCounts.set(base, count);
  const key = `${base} ${count}`;
  const message = ops.test_snapshot(activeCase, key, snapshotValue(actual), kind);
  if (message !== undefined) throw message;
}

// An inline snapshot: compared against the value written in the call, with
// the indentation the writer added taken off first. The matcher's own stack
// goes to the host, which finds the call in the source from its first frame
// outside `runtime:test` — to write the value there when there is none yet.
function inlineSnapshot(serialized, inline) {
  if (activeCase === null) throw new Error("toMatchInlineSnapshot must run inside a test");
  if (inline !== undefined && typeof inline !== "string") {
    throw new TypeError("an inline snapshot is a string");
  }
  const existing = inline === undefined ? null : stripIndentation(inline);
  const message = ops.test_inline_snapshot(activeCase, String(new Error().stack), serialized, existing);
  if (message !== undefined) throw message;
}

// Takes off the indentation a multi-line inline snapshot is written with: a
// literal that opens and closes on lines of its own, every line in between
// indented at least as far as the first. Anything else is compared as written.
// The same rule Jest and Vitest apply, so their snapshots read the same here.
function stripIndentation(text) {
  const lines = text.split("\n");
  if (lines.length <= 2) return text;
  if (lines[0].trim() !== "" || lines.at(-1).trim() !== "") return text;
  const indentation = lines[1].match(/^[ \t]*/)[0];
  const body = lines.slice(1, -1);
  if (body.some((line) => line !== "" && !line.startsWith(indentation))) return text;
  return body.map((line) => line.slice(indentation.length)).join("\n");
}

// The assert spelling is useful to helpers that deliberately avoid constructing
// an expectation chain. It shares the same active-case key and host store.
function assertSnapshot(actual, name) {
  counted();
  snapshot(actual, name);
}

const showError = (e) =>
  e && e.name && e.message !== undefined ? `${e.name}: ${e.message}` : String(e);

const showExpected = (want) =>
  typeof want === "function" ? want.name || "the expected error" : String(want);

// What the second argument to assertThrows/assertRejects means: an error name or
// a substring of its message, a RegExp over the message, or a constructor for an
// instanceof check.
function matches(err, want) {
  if (want === undefined || want === null) return true;
  const message = err && err.message !== undefined ? String(err.message) : String(err);
  const name = err && err.name ? String(err.name) : "";
  if (typeof want === "string") return name === want || message.includes(want);
  if (want instanceof RegExp) return want.test(message) || want.test(showError(err));
  if (typeof want === "function") return err instanceof want;
  return false;
}

function checkThrew(err, want, message, verb, connective) {
  if (matches(err, want)) return;
  throw new Error(
    `${message ? `${message}: ` : ""}expected it to ${verb} ${connective}${showExpected(want)}` +
      `, got ${showError(err)}`,
  );
}

function neverThrew(want, message, verb, connective) {
  const expectation = want === undefined ? "" : ` ${connective}${showExpected(want)}`;
  throw new Error(
    `${message ? `${message}: ` : ""}expected it to ${verb}${expectation}, but it did not`,
  );
}

function assertEquals(actual, expected, message) {
  counted();
  if (equal(actual, expected, [])) return;
  throw new Error(
    `${message ? `${message}: ` : ""}expected ${show(expected)}, got ${show(actual)}`,
  );
}

// The second argument is what the error must be, not a label. It used to be the
// message printed on failure, which made the natural thing to write —
// `assertThrows(fn, "TypeError")` — assert nothing at all: any throw passed.
function assertThrows(fn, want, message) {
  counted();
  let threw;
  let caught = false;
  try {
    fn();
  } catch (err) {
    threw = err;
    caught = true;
  }
  if (!caught) neverThrew(want, message, "throw", "");
  checkThrew(threw, want, message, "throw", "");
}

async function assertRejects(fn, want, message) {
  counted();
  let threw;
  let caught = false;
  try {
    await fn();
  } catch (err) {
    threw = err;
    caught = true;
  }
  if (!caught) neverThrew(want, message, "reject", "with ");
  checkThrew(threw, want, message, "reject", "with ");
}

// ---------------------------------------------------------------------------
// expect
//
// The vocabulary the ecosystem writes tests in. `assertEquals(a, b)` and
// `expect(a).toEqual(b)` are the same assertion and share the same comparison —
// this is a second spelling, not a second implementation, and the reason to have
// it is that a suite written for any other runner should run here unchanged.
//
// Mocks and fake timers are the other half of the vocabulary, and they are a
// subsystem rather than a matcher — they live under `mock` and `clock`, below.
// ---------------------------------------------------------------------------

const MATCHER = Symbol.for("runtime:test.asymmetric");
const SNAPSHOT_MATCHER = Symbol("runtime:test.snapshotMatcher");

const isMatcher = (v) => v !== null && typeof v === "object" && v[MATCHER] === true;

const matcher = (label, matches) => ({ [MATCHER]: true, label, matches });

function fail(actual, expected, negated, verb) {
  throw new Error(
    negated
      ? `expected ${show(actual)} not to ${verb} ${show(expected)}`
      : `expected ${show(actual)} to ${verb} ${show(expected)}`,
  );
}

// Every matcher is written as "does it hold?", and negation is applied in one
// place. Written the other way — a `not` object with its own inverted
// implementations — is how a suite ends up with a matcher whose negation does
// not mean what it says.
function check(held, negated, report) {
  if (held !== negated) return;
  report();
}

const lengthOf = (v) =>
  v === null || v === undefined
    ? undefined
    : typeof v.length === "number"
      ? v.length
      : typeof v.size === "number"
        ? v.size
        : undefined;

function contains(actual, wanted) {
  if (typeof actual === "string") {
    return typeof wanted === "string" && actual.includes(wanted);
  }
  if (actual instanceof Set || actual instanceof Map) {
    return actual.has(wanted);
  }
  if (actual !== null && typeof actual === "object" && typeof actual.length === "number") {
    return Array.prototype.some.call(actual, (v) => Object.is(v, wanted) || v === wanted);
  }
  return false;
}

// `toMatchObject`: every key the *expectation* names, compared structurally, and
// nothing said about the keys it does not name.
function matchesObject(actual, expected, seen) {
  if (isMatcher(expected)) return expected.matches(actual);
  if (Array.isArray(expected)) {
    if (!Array.isArray(actual) || actual.length !== expected.length) return false;
    return expected.every((want, i) => matchesObject(actual[i], want, seen));
  }
  if (expected === null || typeof expected !== "object") return equal(actual, expected, seen);
  if (actual === null || typeof actual !== "object") return false;
  return Object.keys(expected).every(
    (key) =>
      Object.prototype.hasOwnProperty.call(actual, key) &&
      matchesObject(actual[key], expected[key], seen),
  );
}

function property(actual, path) {
  const parts = Array.isArray(path) ? path : String(path).split(".");
  let at = actual;
  for (const part of parts) {
    if (at === null || at === undefined) return { found: false };
    if (!(part in Object(at))) return { found: false };
    at = at[part];
  }
  return { found: true, value: at };
}

async function threw(fn) {
  try {
    const result = typeof fn === "function" ? fn() : fn;
    if (result && typeof result.then === "function") await result;
  } catch (err) {
    return { caught: true, err };
  }
  return { caught: false };
}

/// A mock's record, or a complaint that this is not a mock at all.
function recordOf(value, matcher) {
  if (!isMock(value)) {
    throw new TypeError(
      `expect(...).${matcher} needs a mock — mock.fn() or mock.spyOn(), and ` +
        `${show(value)} is neither`,
    );
  }
  return value.mock;
}

const callsOf = (value, matcher) => recordOf(value, matcher).calls;
const resultsOf = (value, matcher) => recordOf(value, matcher).results;
const settledOf = (value, matcher) => recordOf(value, matcher).settledResults;

/// Whether `before`'s first call came before `after`'s. A `before` never called
/// counts as first unless the test says it must have been.
function calledFirst(before, after, requireCall) {
  const first = before.mock.invocationCallOrder;
  const second = after.mock.invocationCallOrder;
  if (first.length === 0) return !requireCall;
  if (second.length === 0) return false;
  return first[0] < second[0];
}

/// The other mock a call-order matcher compares with, or a complaint.
function otherMock(value, matcher) {
  if (!isMock(value)) {
    throw new TypeError(`expect(...).${matcher} needs another mock to compare with, and ${show(value)} is not one`);
  }
  return value;
}

/// The calls a mock has seen, short enough to read in a failure.
const showCalls = (calls) =>
  calls.length === 0 ? "not called" : calls.map((call) => show(call)).join(", ");

/// A failure about a mock, which names it: a suite with six spies all reporting
/// "expected [Function] to have been called" says nothing about which one.
function called(mock, what, expected, negated) {
  const name = isMock(mock) ? mock.getMockName() : "the function";
  throw new Error(
    expected === undefined
      ? `expected ${name} ${what}`
      : negated
        ? `expected ${name} not to have been called with ${show(expected)}, and it was: ${what}`
        : `expected ${name} to have been called with ${show(expected)}, and it was ${what}`,
  );
}

function throwsSync(fn) {
  try {
    fn();
  } catch (err) {
    return { caught: true, err };
  }
  return { caught: false };
}

// The node a DOM matcher was given, or a `TypeError` naming the matcher.
// `toBeInTheDocument` alone accepts `null`, which is how a query that found
// nothing is asserted to have found nothing.
function nodeOf(actual, name, nullable = false) {
  if (nullable && actual === null) return null;
  if (actual === null || typeof actual !== "object" || typeof actual.nodeType !== "number") {
    throw new TypeError(`expect(...).${name} needs a DOM node, and ${show(actual)} is not one`);
  }
  return actual;
}

// `<button id="save" class="primary">` — a node as a failure names it.
function describeNode(node) {
  if (node === null || node === undefined) return String(node);
  if (typeof node !== "object" || typeof node.nodeType !== "number") return show(node);
  if (node.nodeType !== 1) return node.nodeName;
  const id = node.id ? ` id="${node.id}"` : "";
  const className = node.getAttribute("class") ? ` class="${node.getAttribute("class")}"` : "";
  return `<${node.localName}${id}${className}>`;
}

function dom(node, negated, what, found) {
  throw new Error(
    `expected ${describeNode(node)} ${negated ? "not " : ""}to ${what}${found === undefined ? "" : ` — found ${found}`}`,
  );
}

// Visible: in the document, and neither it nor anything it is inside is
// `display: none`, `hidden`, zero opacity, or — for itself, since it
// inherits — `visibility: hidden`/`collapse`.
function visible(node) {
  if (!node.isConnected) return false;
  const view = node.ownerDocument.defaultView ?? globalThis;
  const style = view.getComputedStyle(node);
  if (style.visibility === "hidden" || style.visibility === "collapse") return false;
  for (let at = node; at; at = at.parentNode ?? at.host ?? null) {
    if (at.nodeType !== 1) continue;
    if (at.hasAttribute("hidden")) return false;
    const own = view.getComputedStyle(at);
    if (own.display === "none" || own.opacity === "0") return false;
  }
  return true;
}

// What a form control's value is, in the type a test would write it in.
function valueOf(node) {
  const name = node.localName;
  if (name === "input") {
    const type = (node.type ?? "text").toLowerCase();
    if (type === "checkbox" || type === "radio") {
      throw new TypeError("expect(...).toHaveValue: a checkbox or radio's state is toBeChecked()");
    }
    if (type === "number" || type === "range") return node.value === "" ? null : Number(node.value);
    return node.value;
  }
  if (name === "select" && node.multiple) {
    return Array.from(node.options).filter((option) => option.selected).map((option) => option.value);
  }
  if (name === "select" || name === "textarea" || name === "button" || name === "output") return node.value;
  if ("value" in node) return node.value;
  throw new TypeError(`expect(...).toHaveValue needs a form control, and ${describeNode(node)} is not one`);
}

function checked(node) {
  if (node.localName === "input" && (node.type === "checkbox" || node.type === "radio")) return node.checked;
  const role = node.getAttribute("role");
  if (role === "checkbox" || role === "radio" || role === "switch" || role === "menuitemcheckbox" || role === "menuitemradio") {
    return node.getAttribute("aria-checked") === "true";
  }
  throw new TypeError(
    `expect(...).toBeChecked needs a checkbox, a radio, or an element with a checkable role, and ${describeNode(node)} is none of them`,
  );
}

// Disabled as the HTML specification decides it: the control's own
// attribute, or a disabled `<fieldset>` around it — except inside that
// fieldset's first `<legend>`, which stays usable.
const DISABLEABLE = new Set(["button", "input", "select", "textarea", "optgroup", "option", "fieldset"]);
function disabled(node) {
  if (!DISABLEABLE.has(node.localName)) return false;
  if (node.hasAttribute("disabled")) return true;
  if (node.localName === "option" && node.parentElement?.localName === "optgroup" && node.parentElement.hasAttribute("disabled")) {
    return true;
  }
  let inside = node;
  for (let at = node.parentElement; at; inside = at, at = at.parentElement) {
    if (at.localName !== "fieldset" || !at.hasAttribute("disabled")) continue;
    const legend = Array.from(at.children).find((child) => child.localName === "legend");
    if (legend !== inside) return true;
  }
  return false;
}

// `mode` is how a failed matcher is handled: `"hard"` throws, `"soft"` records
// it against the running case and carries on, and `"quiet"` throws without
// counting an assertion — for `expect.poll`, which retries a matcher and counts
// the assertion once.
function expectation(actual, negated, mode = "hard") {
  const it = {
    toBe(expected) {
      check(Object.is(actual, expected), negated, () => fail(actual, expected, negated, "be"));
    },
    toEqual(expected) {
      check(equal(actual, expected, []), negated, () => fail(actual, expected, negated, "equal"));
    },
    // `toEqual`, and also: a key set to `undefined` is not a key left out, a
    // hole in an array is not an `undefined`, and a class instance is not a
    // plain object with the same fields.
    toStrictEqual(expected) {
      check(equal(actual, expected, [], true), negated, () =>
        fail(actual, expected, negated, "strictly equal"),
      );
    },
    toMatchSnapshot(name) {
      if (negated) throw new TypeError("expect(...).not.toMatchSnapshot is not meaningful");
      snapshot(actual, name);
    },
    toMatchFileSnapshot(name) {
      if (negated) throw new TypeError("expect(...).not.toMatchFileSnapshot is not meaningful");
      if (activeCase === null) throw new Error("toMatchFileSnapshot must run inside a test");
      if (typeof name !== "string") throw new TypeError("toMatchFileSnapshot(name) needs a filename");
      const message = ops.test_file_snapshot(activeCase, name, actual);
      if (message !== undefined) throw message;
    },
    toThrowErrorMatchingSnapshot(name) {
      if (negated) throw new TypeError("expect(...).not.toThrowErrorMatchingSnapshot is not meaningful");
      if (typeof actual !== "function") throw new TypeError("expect(...).toThrowErrorMatchingSnapshot needs a function");
      const result = throwsSync(actual);
      if (!result.caught) throw new Error("expected function to throw");
      snapshot(result.err, name, "error");
    },
    // The snapshot kept in the call itself, as the ecosystem writes it:
    // `toMatchInlineSnapshot(propertyMatchers?, snapshot?)`. With no snapshot
    // yet, a local run writes one into the source; `--ci` refuses.
    toMatchInlineSnapshot(first, second) {
      if (negated) throw new TypeError("expect(...).not.toMatchInlineSnapshot is not meaningful");
      let value = actual;
      let inline = second;
      if (typeof first === "string") {
        inline = first;
      } else if (first !== undefined) {
        if (first === null || typeof first !== "object") {
          throw new TypeError("toMatchInlineSnapshot(propertyMatchers?, snapshot?) takes an object of property matchers");
        }
        if (!matchesObject(actual, first, [])) throw new Error("snapshot value did not satisfy its property matchers");
        value = maskSnapshot(actual, first);
      }
      inlineSnapshot(snapshotValue(value), inline);
    },
    toThrowErrorMatchingInlineSnapshot(inline) {
      if (negated) throw new TypeError("expect(...).not.toThrowErrorMatchingInlineSnapshot is not meaningful");
      if (typeof actual !== "function") throw new TypeError("expect(...).toThrowErrorMatchingInlineSnapshot needs a function");
      const result = throwsSync(actual);
      if (!result.caught) throw new Error("expected function to throw");
      inlineSnapshot(snapshotValue(result.err), inline);
    },
    toBeTruthy() {
      check(Boolean(actual), negated, () =>
        fail(actual, "truthy", negated, "be"),
      );
    },
    toBeFalsy() {
      check(!actual, negated, () => fail(actual, "falsy", negated, "be"));
    },
    toBeNull() {
      check(actual === null, negated, () => fail(actual, null, negated, "be"));
    },
    toBeNullable() {
      check(actual === null || actual === undefined, negated, () =>
        { throw new Error(`expected ${show(actual)} ${negated ? "not " : ""}to be null or undefined`); },
      );
    },
    toBeUndefined() {
      check(actual === undefined, negated, () => fail(actual, undefined, negated, "be"));
    },
    toBeDefined() {
      check(actual !== undefined, negated, () => fail(actual, "defined", negated, "be"));
    },
    toBeNaN() {
      check(Number.isNaN(actual), negated, () => fail(actual, NaN, negated, "be"));
    },
    toBeInstanceOf(constructor) {
      check(actual instanceof constructor, negated, () =>
        fail(actual, constructor?.name ?? constructor, negated, "be an instance of"),
      );
    },
    toBeTypeOf(type) {
      check(typeof actual === type, negated, () => fail(actual, type, negated, "be of type"));
    },
    toContain(wanted) {
      check(contains(actual, wanted), negated, () => fail(actual, wanted, negated, "contain"));
    },
    toContainEqual(wanted) {
      const held =
        actual !== null &&
        typeof actual === "object" &&
        Array.prototype.some.call(actual, (v) => equal(v, wanted, []));
      check(held, negated, () => fail(actual, wanted, negated, "contain an equal"));
    },
    toHaveLength(length) {
      check(lengthOf(actual) === length, negated, () =>
        fail(lengthOf(actual), length, negated, "have a length of"),
      );
    },
    toHaveProperty(path, ...value) {
      const found = property(actual, path);
      const held = value.length === 0 ? found.found : found.found && equal(found.value, value[0], []);
      check(held, negated, () =>
        fail(actual, value.length === 0 ? path : `${path} = ${show(value[0])}`, negated, "have"),
      );
    },
    toMatch(pattern) {
      const text = String(actual);
      const held = pattern instanceof RegExp ? pattern.test(text) : text.includes(String(pattern));
      check(held, negated, () => fail(actual, pattern, negated, "match"));
    },
    toMatchObject(expected) {
      check(matchesObject(actual, expected, []), negated, () =>
        fail(actual, expected, negated, "match"),
      );
    },
    toBeGreaterThan(n) {
      check(actual > n, negated, () => fail(actual, n, negated, "be greater than"));
    },
    toBeGreaterThanOrEqual(n) {
      check(actual >= n, negated, () =>
        fail(actual, n, negated, "be greater than or equal to"),
      );
    },
    toBeLessThan(n) {
      check(actual < n, negated, () => fail(actual, n, negated, "be less than"));
    },
    toBeLessThanOrEqual(n) {
      check(actual <= n, negated, () => fail(actual, n, negated, "be less than or equal to"));
    },
    // Two digits by default, as everywhere else this is spelled: the point of
    // the matcher is floating-point noise, not a tolerance anybody remembers.
    toBeCloseTo(n, digits = 2) {
      const held = Math.abs(actual - n) < 10 ** -digits / 2;
      check(held, negated, () => fail(actual, n, negated, `be close to (${digits} digits)`));
    },
    // --- what a mock was asked ---
    //
    // Every one of these needs `actual` to be a mock, and says so rather than
    // reporting that `undefined` is not what was expected: a matcher applied to
    // a plain function is a mistake in the test, not a failing assertion.
    toHaveBeenCalled() {
      const calls = callsOf(actual, "toHaveBeenCalled");
      check(calls.length > 0, negated, () =>
        called(actual, negated ? `to have been called ${calls.length} time(s)` : "never to have been called"),
      );
    },
    toHaveBeenCalledTimes(n) {
      const calls = callsOf(actual, "toHaveBeenCalledTimes");
      check(calls.length === n, negated, () =>
        fail(calls.length, n, negated, "have been called this many times:"),
      );
    },
    toHaveBeenCalledOnce() {
      const calls = callsOf(actual, "toHaveBeenCalledOnce");
      check(calls.length === 1, negated, () =>
        fail(calls.length, 1, negated, "have been called this many times:"),
      );
    },
    toHaveBeenCalledWith(...args) {
      const calls = callsOf(actual, "toHaveBeenCalledWith");
      check(calls.some((call) => equal(call, args, [])), negated, () =>
        called(actual, `called with ${showCalls(calls)}`, args, negated),
      );
    },
    toHaveBeenLastCalledWith(...args) {
      const calls = callsOf(actual, "toHaveBeenLastCalledWith");
      const last = calls.at(-1);
      check(calls.length > 0 && equal(last, args, []), negated, () =>
        called(actual, `last called with ${show(last)}`, args, negated),
      );
    },
    // 1-based, as everywhere this matcher is spelled: the first call is 1.
    toHaveBeenNthCalledWith(n, ...args) {
      const calls = callsOf(actual, "toHaveBeenNthCalledWith");
      const call = calls[n - 1];
      check(n >= 1 && n <= calls.length && equal(call, args, []), negated, () =>
        called(actual, `call ${n} was ${show(call)}`, args, negated),
      );
    },
    // Every answer of a `mock.when` chain used: `times` of them, or at least
    // once for one with no limit.
    toHaveBeenExhausted() {
      const chain = actual?.[WHEN];
      if (!chain) throw new TypeError("toHaveBeenExhausted needs a mock.when(…) chain");
      const left = chain.answers.filter((a) => a.used < (a.times === Infinity ? 1 : a.times));
      check(chain.answers.length > 0 && left.length === 0, negated, () =>
        called(
          chain.spy,
          negated
            ? "not to have used every answer"
            : chain.answers.length === 0
              ? "to have answers, and it has none"
              : `to have used every answer; left: ${left
                  .map((a) => `calledWith${showArgs(a.args)}.${a.verb} (${a.used} of ${a.times === Infinity ? 1 : a.times})`)
                  .join(", ")}`,
        ),
      );
    },
    toHaveBeenCalledExactlyOnceWith(...args) {
      const calls = callsOf(actual, "toHaveBeenCalledExactlyOnceWith");
      check(calls.length === 1 && equal(calls[0], args, []), negated, () =>
        called(actual, `called ${calls.length} time(s): ${showCalls(calls)}`, args, negated),
      );
    },
    toHaveBeenCalledBefore(other, requireCall = true) {
      recordOf(actual, "toHaveBeenCalledBefore");
      const after = otherMock(other, "toHaveBeenCalledBefore");
      check(calledFirst(actual, after, requireCall), negated, () =>
        called(actual, `${negated ? "not " : ""}to have been called before ${after.getMockName()}`),
      );
    },
    toHaveBeenCalledAfter(other, requireCall = true) {
      recordOf(actual, "toHaveBeenCalledAfter");
      const before = otherMock(other, "toHaveBeenCalledAfter");
      check(calledFirst(before, actual, requireCall), negated, () =>
        called(actual, `${negated ? "not " : ""}to have been called after ${before.getMockName()}`),
      );
    },
    // What returned promises came to. One not settled yet has not resolved:
    // await the call before asserting.
    toHaveResolved() {
      const settled = settledOf(actual, "toHaveResolved");
      check(settled.some((r) => r.type === "fulfilled"), negated, () =>
        called(actual, negated ? "not to have resolved" : "to have resolved at least once"),
      );
    },
    toHaveResolvedTimes(n) {
      const settled = settledOf(actual, "toHaveResolvedTimes");
      const count = settled.filter((r) => r.type === "fulfilled").length;
      check(count === n, negated, () => fail(count, n, negated, "have resolved this many times:"));
    },
    toHaveResolvedWith(value) {
      const settled = settledOf(actual, "toHaveResolvedWith");
      const values = settled.filter((r) => r.type === "fulfilled").map((r) => r.value);
      check(values.some((v) => equal(v, value, [])), negated, () =>
        called(actual, `${negated ? "not " : ""}to have resolved with ${show(value)}; it resolved with ${show(values)}`),
      );
    },
    toHaveLastResolvedWith(value) {
      const last = settledOf(actual, "toHaveLastResolvedWith").at(-1);
      const held = last?.type === "fulfilled" && equal(last.value, value, []);
      check(held, negated, () => fail(last?.value, value, negated, "have last resolved"));
    },
    toHaveNthResolvedWith(n, value) {
      const at = settledOf(actual, "toHaveNthResolvedWith")[n - 1];
      const held = at?.type === "fulfilled" && equal(at.value, value, []);
      check(held, negated, () => fail(at?.value, value, negated, `have resolved on call ${n}:`));
    },
    toHaveReturned() {
      const results = resultsOf(actual, "toHaveReturned");
      check(results.some((r) => r.type === "return"), negated, () =>
        called(actual, negated ? "to have returned" : "never to have returned without throwing"),
      );
    },
    toHaveReturnedTimes(n) {
      const results = resultsOf(actual, "toHaveReturnedTimes");
      const returned = results.filter((r) => r.type === "return").length;
      check(returned === n, negated, () =>
        fail(returned, n, negated, "have returned this many times:"),
      );
    },
    toHaveReturnedWith(value) {
      const results = resultsOf(actual, "toHaveReturnedWith");
      const held = results.some((r) => r.type === "return" && equal(r.value, value, []));
      check(held, negated, () => fail(actual, value, negated, "have returned"));
    },
    toHaveLastReturnedWith(value) {
      const results = resultsOf(actual, "toHaveLastReturnedWith");
      const last = results.at(-1);
      const held = last?.type === "return" && equal(last.value, value, []);
      check(held, negated, () => fail(last?.value, value, negated, "have last returned"));
    },
    toHaveNthReturnedWith(n, value) {
      const results = resultsOf(actual, "toHaveNthReturnedWith");
      const at = results[n - 1];
      const held = at?.type === "return" && equal(at.value, value, []);
      check(held, negated, () => fail(at?.value, value, negated, `have returned on call ${n}:`));
    },
    toThrow(want) {
      if (typeof actual !== "function") {
        throw new TypeError("expect(...).toThrow needs a function to call");
      }
      const outcome = throwsSync(actual);
      if (negated) {
        if (outcome.caught && matches(outcome.err, want)) {
          throw new Error(
            `expected it not to throw${want === undefined ? "" : ` ${showExpected(want)}`}` +
              `, and it threw ${showError(outcome.err)}`,
          );
        }
        return;
      }
      if (!outcome.caught) neverThrew(want, "", "throw", "");
      checkThrew(outcome.err, want, "", "throw", "");
    },
    toSatisfy(predicate, message) {
      if (typeof predicate !== "function") throw new TypeError("expect(...).toSatisfy needs a predicate");
      check(Boolean(predicate(actual)), negated, () => {
        throw new Error(message ?? `expected ${show(actual)} ${negated ? "not " : ""}to satisfy ${predicate.name || "the predicate"}`);
      });
    },
    toBeOneOf(options) {
      if (!Array.isArray(options)) throw new TypeError("expect(...).toBeOneOf needs an array");
      check(options.some((option) => equal(actual, option, [])), negated, () =>
        fail(actual, options, negated, "be one of"),
      );
    },
    // --- the DOM ---
    //
    // The questions a component test asks of a node, answered from the DOM
    // alone, so they mean the same under `--dom` and in a browser. Each needs a
    // node and says so, rather than reporting that `null` lacks a class — a
    // query that found nothing is the likeliest cause, and the message should
    // point at it.
    toBeInTheDocument() {
      const node = nodeOf(actual, "toBeInTheDocument", true);
      check(node !== null && node.isConnected, negated, () =>
        dom(node, negated, "be in the document"),
      );
    },
    toBeVisible() {
      const node = nodeOf(actual, "toBeVisible");
      check(visible(node), negated, () => dom(node, negated, "be visible"));
    },
    toBeEmptyDOMElement() {
      const node = nodeOf(actual, "toBeEmptyDOMElement");
      const empty = Array.prototype.every.call(node.childNodes, (child) => child.nodeType === 8);
      check(empty, negated, () => dom(node, negated, "be empty", node.innerHTML));
    },
    toContainElement(element) {
      const node = nodeOf(actual, "toContainElement");
      const held = element !== null && element !== undefined && node.contains(element);
      check(held, negated, () => dom(node, negated, `contain ${describeNode(element)}`));
    },
    toContainHTML(html) {
      const node = nodeOf(actual, "toContainHTML");
      const probe = node.ownerDocument.createElement("div");
      probe.innerHTML = String(html);
      check(node.outerHTML.includes(probe.innerHTML), negated, () =>
        dom(node, negated, `contain the HTML ${JSON.stringify(String(html))}`, node.outerHTML),
      );
    },
    toHaveTextContent(text, { normalizeWhitespace = true } = {}) {
      const node = nodeOf(actual, "toHaveTextContent");
      const raw = node.textContent ?? "";
      const content = normalizeWhitespace ? raw.replace(/\s+/g, " ").trim() : raw;
      const held = text instanceof RegExp ? text.test(content) : content.includes(String(text));
      check(held, negated, () =>
        dom(node, negated, `have the text ${text instanceof RegExp ? text : JSON.stringify(String(text))}`, JSON.stringify(content)),
      );
    },
    toHaveAttribute(name, ...value) {
      const node = nodeOf(actual, "toHaveAttribute");
      const present = node.hasAttribute(name);
      const held = value.length === 0 ? present : present && equal(node.getAttribute(name), value[0], []);
      const what = value.length === 0 ? `have the attribute ${name}` : `have ${name}=${show(value[0])}`;
      check(held, negated, () =>
        dom(node, negated, what, present ? `${name}=${show(node.getAttribute(name))}` : `no ${name}`),
      );
    },
    toHaveClass(...names) {
      const node = nodeOf(actual, "toHaveClass");
      const last = names.at(-1);
      const exact = last !== null && typeof last === "object" && last.exact === true;
      if (last !== null && typeof last === "object") names = names.slice(0, -1);
      const wanted = names.flatMap((name) => String(name).split(/\s+/)).filter(Boolean);
      const have = Array.from(node.classList);
      const held =
        wanted.length === 0
          ? have.length > 0
          : exact
            ? have.length === wanted.length && wanted.every((name) => have.includes(name))
            : wanted.every((name) => have.includes(name));
      const what = wanted.length === 0 ? "have a class" : `have the class${wanted.length > 1 ? "es" : ""} ${wanted.join(" ")}${exact ? " and no others" : ""}`;
      check(held, negated, () => dom(node, negated, what, `class="${have.join(" ")}"`));
    },
    toHaveValue(value) {
      const node = nodeOf(actual, "toHaveValue");
      const have = valueOf(node);
      check(equal(have, value, []), negated, () => dom(node, negated, `have the value ${show(value)}`, show(have)));
    },
    toBeChecked() {
      const node = nodeOf(actual, "toBeChecked");
      check(checked(node), negated, () => dom(node, negated, "be checked"));
    },
    toBeDisabled() {
      const node = nodeOf(actual, "toBeDisabled");
      check(disabled(node), negated, () => dom(node, negated, "be disabled"));
    },
    toBeEnabled() {
      const node = nodeOf(actual, "toBeEnabled");
      check(!disabled(node), negated, () => dom(node, negated, "be enabled"));
    },
    toBeRequired() {
      const node = nodeOf(actual, "toBeRequired");
      const held = node.required === true || node.getAttribute("aria-required") === "true";
      check(held, negated, () => dom(node, negated, "be required"));
    },
    toHaveFocus() {
      const node = nodeOf(actual, "toHaveFocus");
      const root = node.getRootNode();
      const held = root.activeElement === node || node.ownerDocument.activeElement === node;
      check(held, negated, () =>
        dom(node, negated, "have focus", `focus is on ${describeNode(node.ownerDocument.activeElement)}`),
      );
    },
    // Each property as the node's computed style has it, against the value
    // written — itself put through the browser's own parser first, so `0` and
    // `0px` are one value. Computed values are what is compared: a colour
    // computes to `rgb()`, and is written that way here.
    toHaveStyle(css) {
      const node = nodeOf(actual, "toHaveStyle");
      const view = node.ownerDocument.defaultView ?? globalThis;
      const computed = view.getComputedStyle(node);
      const probe = node.ownerDocument.createElement("div");
      if (typeof css === "string") {
        probe.style.cssText = css;
      } else if (css !== null && typeof css === "object") {
        for (const [key, value] of Object.entries(css)) {
          const property = key.startsWith("--") ? key : key.replace(/[A-Z]/g, (c) => `-${c.toLowerCase()}`);
          probe.style.setProperty(property, String(value));
        }
      } else {
        throw new TypeError("expect(...).toHaveStyle needs a CSS string or an object of properties");
      }
      const wanted = Array.from(probe.style, (property) => [property, probe.style.getPropertyValue(property)]);
      if (wanted.length === 0) throw new TypeError(`expect(...).toHaveStyle: no valid declarations in ${show(css)}`);
      const differing = wanted.filter(([property, value]) => computed.getPropertyValue(property) !== value);
      check(differing.length === 0, negated, () =>
        dom(
          node,
          negated,
          `have the style ${wanted.map(([p, v]) => `${p}: ${v}`).join("; ")}`,
          (negated ? wanted : differing).map(([p]) => `${p}: ${computed.getPropertyValue(p)}`).join("; "),
        ),
      );
    },
  };
  it.toThrowError = it.toThrow;
  // The shorter spellings, which are the same matchers under the names jest
  // gave them first. Aliases rather than copies: two implementations of one
  // assertion is how they end up disagreeing.
  it.toBeCalled = it.toHaveBeenCalled;
  it.toBeCalledTimes = it.toHaveBeenCalledTimes;
  it.toBeCalledWith = it.toHaveBeenCalledWith;
  it.lastCalledWith = it.toHaveBeenLastCalledWith;
  it.nthCalledWith = it.toHaveBeenNthCalledWith;
  it.toReturn = it.toHaveReturned;
  it.toReturnTimes = it.toHaveReturnedTimes;
  it.toReturnWith = it.toHaveReturnedWith;
  it.lastReturnedWith = it.toHaveLastReturnedWith;
  it.nthReturnedWith = it.toHaveNthReturnedWith;
  for (const [name, fn] of custom) it[name] = (...args) => extended(name, fn, actual, negated, args);
  // Wrapped once, here, rather than in each matcher: every matcher counts as
  // an assertion and fails the same way, built in or added by `expect.extend`.
  // The wrappers go on a new object so a matcher that calls another through
  // `it` (`toStrictEqual`) counts as the one assertion it is.
  const out = {};
  for (const [name, run] of Object.entries(it)) {
    out[name] = (...args) => {
      if (mode !== "quiet") counted();
      if (mode !== "soft") return run(...args);
      try {
        const result = run(...args);
        return result && typeof result.then === "function" ? result.catch(softly) : result;
      } catch (err) {
        softly(err);
      }
    };
  }
  return out;
}

// A soft failure: recorded against the running case, which fails when it ends.
function softly(err) {
  if (attempt === null) throw err;
  attempt.soft.push(err);
}

// Matchers added by `expect.extend`, by name.
const custom = new Map();

// The object a custom matcher is called with as `this`, as the ecosystem's
// matchers expect to find it.
const matcherContext = (negated) => ({
  isNot: negated,
  equals: (a, b) => equal(a, b, []),
  utils: { stringify: show, printReceived: show, printExpected: show },
});

// Runs a custom matcher and applies its verdict. One that returns a promise is
// awaited, so an async matcher works — and has to be awaited by the test.
function extended(name, fn, actual, negated, args) {
  const verdict = (result) => {
    if (result === null || typeof result !== "object" || typeof result.pass !== "boolean") {
      throw new TypeError(`expect.extend: ${name} must return { pass: boolean, message: () => string }`);
    }
    if (result.pass !== negated) return;
    const message = typeof result.message === "function" ? result.message() : result.message;
    throw new Error(message || `expected ${show(actual)} ${negated ? "not " : ""}to pass ${name}`);
  };
  const result = fn.call(matcherContext(negated), actual, ...args);
  if (result && typeof result.then === "function") return result.then(verdict);
  verdict(result);
}

// `await expect(promise).resolves.toEqual(x)` — the promise is settled first and
// the matcher runs on what came out of it, so a rejection is reported as one
// rather than as a mismatched Promise object.
function awaited(promise, negated, wantResolved, mode = "hard") {
  const handler = {
    get(_target, name) {
      // `.resolves.not.toBe(x)` — negation reached through the proxy, which
      // otherwise answers every name with a matcher and would hand back an
      // async function called `not`.
      if (name === "not") return awaited(promise, !negated, wantResolved, mode);
      const run = async (...args) => {
        const outcome = await threw(promise);
        if (wantResolved && outcome.caught) {
          throw new Error(`expected it to resolve, and it rejected: ${showError(outcome.err)}`);
        }
        if (!wantResolved && !outcome.caught) {
          throw new Error("expected it to reject, and it resolved");
        }
        if (!wantResolved) {
          // `rejects.toThrow(...)` asserts about the error itself, and every
          // other matcher asserts about it as a value.
          const err = outcome.err;
          if (name === "toThrow" || name === "toThrowError") {
            if (negated) {
              if (matches(err, args[0])) {
                throw new Error(
                  `expected it not to reject with ${showExpected(args[0])}` +
                    `, and it did: ${showError(err)}`,
                );
              }
              return;
            }
            checkThrew(err, args[0], "", "reject", "with ");
            return;
          }
          await expectation(err, negated, "quiet")[name](...args);
          return;
        }
        const value = await promise;
        await expectation(value, negated, "quiet")[name](...args);
      };
      // Counted and softened here, once, rather than by the matcher it ends
      // up calling — which is quiet for that reason.
      return (...args) => {
        if (mode !== "quiet") counted();
        const result = run(...args);
        return mode === "soft" ? result.catch(softly) : result;
      };
    },
  };
  return new Proxy({}, handler);
}

function assertion(actual, mode) {
  const it = expectation(actual, false, mode);
  it.not = expectation(actual, true, mode);
  it.resolves = awaited(actual, false, true, mode);
  it.rejects = awaited(actual, false, false, mode);
  it.not.resolves = awaited(actual, true, true, mode);
  it.not.rejects = awaited(actual, true, false, mode);
  return it;
}

function expect(actual) {
  return assertion(actual, "hard");
}

// `expect.soft(value)` — a failed matcher is recorded and the test carries on,
// failing at the end with every one of them. For checking several properties of
// one result and seeing all that are wrong in one run.
expect.soft = (actual) => {
  if (attempt === null) throw new Error("expect.soft must be called inside a test");
  return assertion(actual, "soft");
};

// `expect.assertions(n)` — the running test fails unless exactly `n`
// assertions ran in it. For a test whose assertions sit in callbacks, where a
// callback that never ran would otherwise be a test that passed by asserting
// nothing.
expect.assertions = (n) => {
  if (!Number.isInteger(n) || n < 0) throw new TypeError("expect.assertions(n) needs a whole number");
  duringTest("expect.assertions", () => {}).expected = n;
};

// `expect.hasAssertions()` — at least one.
expect.hasAssertions = () => {
  duringTest("expect.hasAssertions", () => {}).atLeastOne = true;
};

// `expect.unreachable(message?)` — fails where it is reached.
expect.unreachable = (message) => {
  throw new Error(message ?? "expected this line not to be reached");
};

// `expect.addEqualityTesters([tester])` — how `toEqual` and every other deep
// comparison decides a pair it would otherwise compare field by field. For the
// rest of the file.
expect.addEqualityTesters = (added) => {
  if (!Array.isArray(added) || added.some((tester) => typeof tester !== "function")) {
    throw new TypeError("expect.addEqualityTesters needs an array of functions");
  }
  testers.push(...added);
};

// `expect.addSnapshotSerializer({ test, serialize })` — prints the values
// `test` accepts in every snapshot for the rest of the file. The newest is
// tried first. The older `print(value, serialize, indent)` form works too.
expect.addSnapshotSerializer = (plugin) => {
  if (
    typeof plugin?.test !== "function" ||
    (typeof plugin.serialize !== "function" && typeof plugin.print !== "function")
  ) {
    throw new TypeError("expect.addSnapshotSerializer needs { test(value), serialize(value, …) }");
  }
  serializers.unshift(plugin);
};

// `expect.fail(message?)` — fails the test here, as Vitest and Chai spell it.
expect.fail = (message) => {
  throw new Error(message ?? "expect.fail() was called");
};

// `expect.extend({ name(received, ...args) { return { pass, message } } })` —
// matchers of your own, used like the built-in ones: negated with `.not`, on
// `.resolves`/`.rejects`, in `expect.soft`, and as asymmetric matchers inside
// an expected value (`expect.name(...args)`). A name the built-ins use is
// replaced for this file.
expect.extend = (matchers) => {
  if (matchers === null || typeof matchers !== "object") {
    throw new TypeError("expect.extend needs an object of matcher functions");
  }
  for (const [name, fn] of Object.entries(matchers)) {
    if (typeof fn !== "function") throw new TypeError(`expect.extend: ${name} is not a function`);
    custom.set(name, fn);
    // An asymmetric form, unless the name is already one of expect's own.
    if (!Object.hasOwn(builtinStatics, name)) {
      const asymmetric = (negated) => (...args) =>
        matcher(`${negated ? "not." : ""}${name}(${args.map(show).join(", ")})`, (value) => {
          const result = fn.call(matcherContext(negated), value, ...args);
          if (result && typeof result.then === "function") {
            throw new TypeError(`expect.${name}: an async matcher cannot be used inside a value`);
          }
          return result?.pass === !negated;
        });
      expect[name] = asymmetric(false);
      expect.not[name] = asymmetric(true);
    }
  }
};

// `expect.poll(fn, { timeout, interval })` — calls `fn` until the matcher
// holds for what it returns, or the time runs out. For state that settles on
// its own schedule: a DOM an update has not reached yet, a queue being drained.
// Always awaited. Waits on the real clock, so a frozen one does not stop it.
expect.poll = (fn, options = {}) => {
  if (typeof fn !== "function") throw new TypeError("expect.poll needs a function to call");
  const { timeout, interval } = waitOptions("expect.poll", options);
  const polled = (negated) =>
    new Proxy(
      {},
      {
        get(_target, name) {
          if (name === "not") return polled(!negated);
          if (name === "then") return undefined;
          return async (...args) => {
            counted();
            const deadline = realNow() + timeout;
            for (;;) {
              try {
                await expectation(await fn(), negated, "quiet")[name](...args);
                return;
              } catch (err) {
                if (realNow() >= deadline) {
                  throw restated(`${showError(err).replace(/^\w*Error: /, "")} (still failing after ${timeout}ms of polling)`, err);
                }
              }
              await pause(interval);
            }
          };
        },
      },
    );
  return polled(false);
};

// The asymmetric matchers, negated: `expect.not.objectContaining({...})`.
expect.not = {};

// The asymmetric matchers: a value that says what it will accept, usable
// wherever a value goes — including several levels inside an expected object,
// which is the case that cannot be written as an assertion of its own.
expect.anything = () =>
  matcher("anything", (v) => v !== null && v !== undefined);
expect.any = (constructor) =>
  matcher(`any(${constructor?.name ?? constructor})`, (v) => {
    if (constructor === String) return typeof v === "string" || v instanceof String;
    if (constructor === Number) return typeof v === "number" || v instanceof Number;
    if (constructor === Boolean) return typeof v === "boolean" || v instanceof Boolean;
    if (constructor === BigInt) return typeof v === "bigint";
    if (constructor === Symbol) return typeof v === "symbol";
    if (constructor === Function) return typeof v === "function";
    return v instanceof constructor;
  });
expect.stringContaining = (part) =>
  matcher(`stringContaining(${part})`, (v) => typeof v === "string" && v.includes(part));
expect.stringMatching = (pattern) =>
  matcher(`stringMatching(${pattern})`, (v) =>
    typeof v === "string" && (pattern instanceof RegExp ? pattern.test(v) : v.includes(pattern)),
  );
expect.arrayContaining = (wanted) =>
  matcher("arrayContaining", (v) =>
    Array.isArray(v) && wanted.every((want) => v.some((have) => equal(have, want, []))),
  );
expect.objectContaining = (wanted) =>
  matcher("objectContaining", (v) => matchesObject(v, wanted, []));
// An array every element of which matches `item` — a value or a matcher.
expect.arrayOf = (item) =>
  matcher(`arrayOf(${isMatcher(item) ? item.label : show(item)})`, (v) =>
    Array.isArray(v) && v.every((have) => equal(have, item, [])),
  );
// A value a Standard Schema (Zod, Valibot, ArkType, …) accepts. Only a schema
// that validates synchronously can stand inside a value.
expect.schemaMatching = (schema) => {
  const standard = schema?.["~standard"];
  if (typeof standard?.validate !== "function") {
    throw new TypeError("expect.schemaMatching needs a Standard Schema: an object with ~standard.validate");
  }
  return matcher(`schemaMatching(${standard.vendor ?? "schema"})`, (v) => {
    const result = standard.validate(v);
    if (result !== null && typeof result?.then === "function") {
      throw new TypeError("expect.schemaMatching: this schema validates asynchronously, and a value cannot wait for it");
    }
    return !result?.issues;
  });
};
// A number within `digits` decimal places of `n` — `toBeCloseTo`, inside a value.
expect.closeTo = (n, digits = 2) =>
  matcher(`closeTo(${n}, ${digits})`, (v) => typeof v === "number" && Math.abs(v - n) < 10 ** -digits / 2);
for (const name of ["stringContaining", "stringMatching", "arrayContaining", "objectContaining", "arrayOf", "schemaMatching"]) {
  const positive = expect[name];
  expect.not[name] = (...args) => {
    const inner = positive(...args);
    return matcher(`not.${inner.label}`, (v) => !inner.matches(v));
  };
}
// `expect`'s own members, which `expect.extend` does not shadow with an
// asymmetric form of the same name.
const builtinStatics = Object.fromEntries(Object.keys(expect).map((key) => [key, true]));

// `waitFor(fn, { timeout, interval })` — calls `fn` until it returns without
// throwing (or its promise resolves), and returns what it returned. For the
// same unsettled state `expect.poll` is for, when the check is more than one
// matcher. Waits on the real clock.
async function waitFor(fn, options = {}) {
  if (typeof fn !== "function") throw new TypeError("waitFor needs a function to call");
  const { timeout, interval } = waitOptions("waitFor", options);
  const deadline = realNow() + timeout;
  for (;;) {
    try {
      return await fn();
    } catch (err) {
      if (realNow() >= deadline) {
        if (err === null || typeof err !== "object" || typeof err.message !== "string") throw err;
        throw restated(`${err.message} (still failing after ${timeout}ms of waiting)`, err);
      }
    }
    await pause(interval);
  }
}

// One second, checked every 50ms: long enough for a render or a microtask
// chain, short enough that a wait for something that will never happen fails
// quickly.
function waitOptions(name, options) {
  if (options === null || typeof options !== "object") {
    throw new TypeError(`${name}: options are { timeout, interval } in milliseconds`);
  }
  const timeout = options.timeout ?? 1000;
  const interval = options.interval ?? 50;
  if (!(Number.isFinite(timeout) && timeout >= 0) || !(Number.isFinite(interval) && interval >= 0)) {
    throw new TypeError(`${name}: timeout and interval are numbers of milliseconds`);
  }
  return { timeout, interval };
}

const pause = (ms) =>
  new Promise((resolve) => Reflect.apply(realSetTimeout, globalThis, [resolve, ms]));


// ---------------------------------------------------------------------------
// mock, clock — standing in for a function, and for time
//
// A test asserts about what a function *did*, and about code that waits.
// Neither can be written as a matcher: a mock is a function with a record
// attached, and a fake clock replaces the timers a program schedules on.
//
// Imported like everything else — `import { mock, clock } from "runtime:test"`.
// Two namespaces rather than one, because they are two subsystems and naming
// them separately is what lets each verb be short: `clock.advance(100)` says
// what moved, where a single shared object forces `advanceTimersByTime`.
//
// The *methods* on a mock keep the names the ecosystem gave them —
// `mockReturnValue`, `mockClear`, `mock.calls` — for the reason `expect` exists
// here at all: they are the vocabulary the matchers read, and a suite written
// against another runner should need an import line rather than a rewrite.
//
// **The clock is the part with teeth.** `clock.freeze()` swaps `setTimeout`,
// `setInterval`, their cancels, and `Date` on `globalThis` — for the whole
// process, not for one test — and everything scheduled through them then moves
// only when the test says so. Those are standards-defined names being replaced
// at the test's own explicit request, which is the opposite of the runtime
// handing out a vocabulary; nothing here is ambient, and nothing is installed
// unless a file asks for it by importing it.
//
// It is safe for one further reason: a test file is a process (see
// [`crate::test`]), so the swap cannot reach the next file. The runner itself
// never schedules on a timer — it drains on microtasks — so a file that freezes
// the clock and forgets to release it cannot wedge the report: the one task
// boundary the runner does cross between a case and its cleanup goes through
// the `setTimeout` it captured at load, which no swap can reach.
// ---------------------------------------------------------------------------

const MOCK = Symbol.for("runtime:test.mock");
const RESTORE = Symbol.for("runtime:test.restore");

const isMock = (v) => typeof v === "function" && v[MOCK] === true;

// Every mock made in this file, so `restoreAll` can mean it. Strong
// references: the process is one test file, and it ends.
const made = new Set();

/// Calls to any mock so far, numbering each one's `invocationCallOrder`.
let invocations = 0;

/// A function that records what it was called with, and answers however it was
/// told to.
function mockFn(implementation) {
  const once = [];
  let impl = implementation;
  let named = implementation?.name || "the mock";
  let restore = null;

  const blank = () => ({
    calls: [],
    results: [],
    settledResults: [],
    instances: [],
    contexts: [],
    invocationCallOrder: [],
    lastCall: undefined,
  });

  const fn = function (...args) {
    const record = fn.mock;
    record.calls.push(args);
    record.lastCall = args;
    record.invocationCallOrder.push(++invocations);
    record.contexts.push(new.target ? undefined : this);
    if (new.target) record.instances.push(this);
    // What a returned promise came to, filled in when it settles.
    const settled = { type: "incomplete", value: undefined };
    record.settledResults.push(settled);
    const use = once.length > 0 ? once.shift() : impl;
    try {
      const value = use ? Reflect.apply(use, this, args) : undefined;
      record.results.push({ type: "return", value });
      if (value !== null && typeof value?.then === "function") {
        value.then(
          (result) => Object.assign(settled, { type: "fulfilled", value: result }),
          (err) => Object.assign(settled, { type: "rejected", value: err }),
        );
      } else {
        Object.assign(settled, { type: "fulfilled", value });
      }
      return value;
    } catch (err) {
      // Recorded *and* rethrown: a mock that swallowed the throw would send the
      // code under test down a path it does not take in production.
      record.results.push({ type: "throw", value: err });
      Object.assign(settled, { type: "rejected", value: err });
      throw err;
    }
  };

  Object.defineProperty(fn, MOCK, { value: true });
  fn.mock = blank();

  fn.mockImplementation = (f) => ((impl = f), fn);
  fn.mockImplementationOnce = (f) => (once.push(f), fn);
  fn.mockReturnValue = (v) => fn.mockImplementation(() => v);
  fn.mockReturnValueOnce = (v) => fn.mockImplementationOnce(() => v);
  fn.mockReturnThis = () =>
    fn.mockImplementation(function () {
      return this;
    });
  fn.mockResolvedValue = (v) => fn.mockImplementation(() => Promise.resolve(v));
  fn.mockResolvedValueOnce = (v) => fn.mockImplementationOnce(() => Promise.resolve(v));
  fn.mockRejectedValue = (e) => fn.mockImplementation(() => Promise.reject(e));
  fn.mockRejectedValueOnce = (e) => fn.mockImplementationOnce(() => Promise.reject(e));
  fn.mockThrow = (e) =>
    fn.mockImplementation(() => {
      throw e;
    });
  fn.mockThrowOnce = (e) =>
    fn.mockImplementationOnce(() => {
      throw e;
    });
  fn.getMockImplementation = () => impl;
  // Answers with `f` while `callback` runs — until it settles, when it is async.
  fn.withImplementation = (f, callback) => {
    const before = impl;
    impl = f;
    let ran;
    try {
      ran = callback();
    } catch (err) {
      impl = before;
      throw err;
    }
    if (ran !== null && typeof ran?.then === "function") {
      return Promise.resolve(ran).then(
        () => {
          impl = before;
        },
        (err) => {
          impl = before;
          throw err;
        },
      );
    }
    impl = before;
  };

  // Three verbs, and the difference between them is what a test means by
  // "start again": forget the calls, forget how it was told to answer, or stop
  // standing in for the real thing altogether.
  fn.mockClear = () => ((fn.mock = blank()), fn);
  fn.mockReset = () => {
    fn.mockClear();
    once.length = 0;
    impl = implementation;
    return fn;
  };
  fn.mockRestore = () => {
    fn.mockReset();
    if (restore) restore();
    return fn;
  };

  // `using spy = mock.spyOn(…)` puts the method back when the block ends.
  Object.defineProperty(fn, Symbol.dispose, { value: () => void fn.mockRestore() });

  fn.mockName = (name) => ((named = name), fn);
  fn.getMockName = () => named;
  // How `spyOn` says what putting the method back means. Not part of the API a
  // test writes against.
  fn[RESTORE] = (f) => {
    restore = f;
  };

  made.add(fn);
  return fn;
}

/// Replaces one method with a mock that still calls the original.
///
/// Calling through by default is the behaviour worth having: a spy is usually
/// installed to *watch* something work, and one that silently returned
/// `undefined` would change the result of every test that installed it.
/// `.mockImplementation(...)` is how a test says otherwise.
function descriptorFor(object, key) {
  for (let current = object; current !== null; current = Object.getPrototypeOf(current)) {
    const descriptor = Object.getOwnPropertyDescriptor(current, key);
    if (descriptor) return descriptor;
  }
  return undefined;
}

function spyOn(object, key, accessType) {
  if (object === null || (typeof object !== "object" && typeof object !== "function")) {
    throw new TypeError("mock.spyOn needs an object or a function to take the method from");
  }
  const own = Object.getOwnPropertyDescriptor(object, key);
  const descriptor = descriptorFor(object, key);
  if (accessType !== undefined && accessType !== "get" && accessType !== "set") {
    throw new TypeError("mock.spyOn access type must be 'get' or 'set'");
  }
  const original = accessType === undefined ? (own ? own.value : object[key]) : descriptor?.[accessType];
  if (typeof original !== "function") {
    throw new TypeError(`mock.spyOn: ${String(key)} is not a ${accessType ?? "method"} of that object`);
  }
  const spy = mockFn(function (...args) {
    return Reflect.apply(original, this, args);
  });
  spy.mockName(String(key));
  spy[RESTORE](() => {
    // An own property goes back exactly as it was; an inherited one is deleted
    // rather than written, so the prototype's method is found again.
    if (own) Object.defineProperty(object, key, own);
    else delete object[key];
  });
  if (accessType === undefined) {
    Object.defineProperty(object, key, {
      value: spy,
      writable: true,
      configurable: true,
      enumerable: own ? own.enumerable : true,
    });
  } else {
    Object.defineProperty(object, key, {
      get: accessType === "get" ? spy : descriptor?.get,
      set: accessType === "set" ? spy : descriptor?.set,
      configurable: true,
      enumerable: own?.enumerable ?? descriptor?.enumerable ?? true,
    });
  }
  return spy;
}

/// Globals a test replaced, and what was there before.
const stubbed = new Map();

// `runtime:process`, for `mock.env`. A page has no process environment, and a
// specifier that is not a literal is one the browser bundle leaves alone.
const PROCESS = ["runtime", "process"].join(":");
const processModule =
  typeof ops.module_resolve_sync === "function" ? await import(PROCESS) : null;

/// Environment variables a test replaced, and what was there before —
/// `undefined` for one that was not set.
const envStubbed = new Map();

const WHEN = Symbol.for("runtime:test.when");

/// Arguments as the call that passed them is written.
const showArgs = (args) => `(${args.map((arg) => show(arg)).join(", ")})`;

/// Answers a mock gives by its arguments, until the chain is disposed.
function when(spy, options = {}) {
  if (!isMock(spy)) {
    throw new TypeError("mock.when needs a mock: mock.fn() or mock.spyOn()");
  }
  const unmatched = options.onUnmatched ?? "passthrough";
  if (unmatched !== "passthrough" && unmatched !== "throw" && typeof unmatched !== "function") {
    throw new TypeError('mock.when: onUnmatched is "passthrough", "throw" or a function');
  }
  const before = spy.getMockImplementation();
  const answers = [];
  let args = null;

  spy.mockImplementation(function (...called) {
    // The newest answer for these arguments first; a used-up one lets an
    // older one answer.
    for (let i = answers.length - 1; i >= 0; i--) {
      const answer = answers[i];
      if (answer.used < answer.times && equal(called, answer.args, [])) {
        answer.used += 1;
        return answer.give();
      }
    }
    if (unmatched === "throw") {
      throw new Error(`${spy.getMockName()} has no answer for the arguments ${showArgs(called)}`);
    }
    if (unmatched !== "passthrough") return Reflect.apply(unmatched, this, called);
    return before ? Reflect.apply(before, this, called) : undefined;
  });

  const then = (verb, give) => (value, options) => {
    if (args === null) {
      throw new TypeError(`mock.when: call .calledWith(…) before .${verb}(…)`);
    }
    const times = options?.times ?? Infinity;
    if (times !== Infinity && !(Number.isInteger(times) && times > 0)) {
      throw new TypeError(`mock.when: ${verb}'s times is a whole number above 0`);
    }
    answers.push({ args, times, used: 0, verb, give: () => give(value) });
    return chain;
  };
  const once = (verb) => (value) => chain[verb](value, { times: 1 });

  const chain = {
    calledWith(...expected) {
      args = expected;
      return chain;
    },
    thenReturn: then("thenReturn", (value) => value),
    thenThrow: then("thenThrow", (err) => {
      throw err;
    }),
    thenResolve: then("thenResolve", (value) => Promise.resolve(value)),
    thenReject: then("thenReject", (err) => Promise.reject(err)),
    thenReturnOnce: once("thenReturn"),
    thenThrowOnce: once("thenThrow"),
    thenResolveOnce: once("thenResolve"),
    thenRejectOnce: once("thenReject"),
    [Symbol.dispose]() {
      spy.mockImplementation(before);
    },
  };
  Object.defineProperty(chain, WHEN, { value: { spy, answers } });
  return chain;
}

// Mocked modules' exports, by resolved URL: the generated module that stands
// in for each one reads its exports from here (D101).
const MODULE_MOCKS = Symbol.for("runtime:test.moduleMocks");
const mockedModules = () => (globalThis[MODULE_MOCKS] ??= new Map());

// A specifier resolved as an `import` in the calling file would resolve it.
// `base` is that file — esdev's transform passes it, so a call cannot be made
// on the file's behalf from somewhere else.
function resolveModule(name, specifier, base) {
  if (typeof ops.module_resolve_sync !== "function") {
    throw new TypeError(`mock.${name} is not available in browser runs yet`);
  }
  if (typeof specifier !== "string") {
    throw new TypeError(`mock.${name} needs a module specifier, as an import would name it`);
  }
  if (typeof base !== "string") {
    throw new TypeError(
      `mock.${name}(${JSON.stringify(specifier)}) must be called by name in the file, as \`mock.${name}(…)\`, so the specifier resolves from it`,
    );
  }
  const url = ops.module_resolve_sync(specifier, base);
  if (typeof url !== "string") throw new TypeError(`cannot resolve ${JSON.stringify(specifier)}`);
  return url;
}

// The real module beside its mock: the loader keeps this query on the id.
const actualModule = (url) => import(`${url}?esdev-actual`);

const mock = {
  fn: mockFn,
  spyOn,
  is: isMock,
  // What a typed suite writes to tell the checker a real function is a mock.
  // There is nothing to satisfy at runtime, so it is the value itself.
  typed: (value) => value,

  /// Replaces a global for the duration of the file. `restoreAll` undoes it.
  global(name, value) {
    if (!stubbed.has(name)) {
      stubbed.set(name, Object.getOwnPropertyDescriptor(globalThis, name) ?? null);
    }
    Object.defineProperty(globalThis, name, {
      value,
      writable: true,
      configurable: true,
      enumerable: true,
    });
    return mock;
  },

  when,

  /// Sets an environment variable in `runtime:process`'s `env` — `undefined`
  /// removes it. `restoreAll` puts every one back.
  env(name, value) {
    if (!processModule) throw new TypeError("mock.env is not available in browser runs");
    if (typeof name !== "string") throw new TypeError("mock.env needs a variable name");
    const { env, unmask } = processModule;
    if (!envStubbed.has(name)) {
      envStubbed.set(name, Object.hasOwn(env, name) ? unmask(env[name]) : undefined);
    }
    if (value === undefined) delete env[name];
    else env[name] = value;
    return mock;
  },

  /// Replaces a module for everything that imports it afterwards — and, when
  /// called at the top of a test file, for that file's own imports, which run
  /// after it. `factory(importOriginal)` returns the module's exports.
  module(specifier, factory, base) {
    if (typeof factory !== "function") {
      throw new TypeError(
        `mock.module(${JSON.stringify(specifier)}) needs a factory returning the module's exports`,
      );
    }
    const url = resolveModule("module", specifier, base);
    const settle = (exports) => {
      if (exports === null || typeof exports !== "object") {
        throw new TypeError(
          `mock.module(${JSON.stringify(specifier)}): the factory must return an object of exports`,
        );
      }
      mockedModules().set(url, exports);
      ops.test_mock_module(url, JSON.stringify(Object.keys(exports)));
    };
    const made = factory(() => actualModule(url));
    if (made !== null && typeof made?.then === "function") return made.then(settle);
    settle(made);
  },

  /// The real module, whether or not it is mocked.
  importActual(specifier, base) {
    return actualModule(resolveModule("importActual", specifier, base));
  },

  clearAll() {
    for (const fn of made) fn.mockClear();
    return mock;
  },
  resetAll() {
    for (const fn of made) fn.mockReset();
    return mock;
  },
  /// Puts everything back: every spy's method, and every replaced global. The
  /// one call an `afterEach` needs, which is why globals are restored here
  /// rather than by a second verb a test can forget.
  restoreAll() {
    for (const fn of made) fn.mockRestore();
    for (const [name, descriptor] of stubbed) {
      if (descriptor) Object.defineProperty(globalThis, name, descriptor);
      else delete globalThis[name];
    }
    stubbed.clear();
    if (processModule) {
      for (const [name, value] of envStubbed) {
        if (value === undefined) delete processModule.env[name];
        else processModule.env[name] = value;
      }
    }
    envStubbed.clear();
    return mock;
  },
};

// --- what global setup provided ---

/// What global setup passed to `provide`, read once.
let providedValues;

/// `inject(key)` — a value global setup provided, or `undefined`.
function inject(key) {
  if (providedValues === undefined) {
    const text = typeof ops.test_provided === "function" ? ops.test_provided() : undefined;
    providedValues = text === undefined ? {} : JSON.parse(text);
  }
  return Object.hasOwn(providedValues, key) ? providedValues[key] : undefined;
}

// --- the clock ---

/// The installed fake clock, or `null` while time is real.
let frozen = null;

/// How many timers one `runAll` will fire before it decides the queue is not
/// going to end, unless `freeze({ loopLimit })` says otherwise. An interval,
/// or a timeout that reschedules itself, never drains — and a test that hangs
/// teaches nothing.
const RUNAWAY = 10_000;

/// How far apart animation frames are, in milliseconds: 60 a second.
const FRAME = 16;

/// What `freeze` can replace. `queueMicrotask` only when asked: the promise
/// jobs a test awaits are microtasks too, and holding those back is rarely
/// what a test wants.
const FAKEABLE = [
  "setTimeout",
  "clearTimeout",
  "setInterval",
  "clearInterval",
  "setImmediate",
  "clearImmediate",
  "requestAnimationFrame",
  "cancelAnimationFrame",
  "requestIdleCallback",
  "cancelIdleCallback",
  "queueMicrotask",
  "Date",
  "performance",
  "Temporal",
  "Intl",
];

/// The clock the test drives, or a complaint that time is still real.
function ticking(verb) {
  if (!frozen) {
    throw new Error(`clock.${verb} needs the frozen clock — call clock.freeze() first`);
  }
  return frozen;
}

/// The timer that comes next, if it is due by `limit`.
///
/// Ties are broken by the order they were scheduled in, which is the order the
/// platform runs them in and the only one a test can reason about.
function due(state, limit) {
  let found = null;
  for (const timer of state.timers.values()) {
    if (timer.at > limit) continue;
    if (!found || timer.at < found.at || (timer.at === found.at && timer.id < found.id)) {
      found = timer;
    }
  }
  return found;
}

/// Runs one timer, having first decided whether it runs again — and then the
/// microtasks it queued, when those are faked too.
///
/// Rescheduled before it is called, so an interval that cancels itself from
/// inside its own callback actually stops.
function fire(state, timer) {
  state.now = timer.at;
  if (timer.every) timer.at += timer.every;
  else state.timers.delete(timer.id);
  Reflect.apply(timer.callback, undefined, timer.args);
  runJobs(state);
}

/// Faked microtasks, in the order they were queued, including any they queue.
function runJobs(state) {
  for (let ran = 0; state.jobs.length > 0; ran += 1) {
    if (ran >= state.loopLimit) {
      throw new Error(`clock: ${state.loopLimit} microtasks ran and the queue is not draining`);
    }
    state.jobs.shift()();
  }
}

/// Whatever is pending on the microtask queue, run.
///
/// A real macrotask, not `await Promise.resolve()`: a promise chain of unknown
/// depth is only guaranteed to be finished once the queue has drained, and
/// draining it is exactly what yielding to a real timer does.
const settle = () => new Promise((resolve) => Reflect.apply(realSetTimeout, globalThis, [resolve, 0]));

/// What `freeze` was asked to replace: a moment, or `{ now, toFake,
/// toNotFake, loopLimit }`.
function freezeOptions(input) {
  const isOptions =
    input !== null && typeof input === "object" && !(input instanceof Date);
  const options = isOptions ? input : { now: input };
  if (options.toFake !== undefined && options.toNotFake !== undefined) {
    throw new TypeError("clock.freeze takes toFake or toNotFake, not both");
  }
  for (const key of ["toFake", "toNotFake"]) {
    const names = options[key];
    if (names === undefined) continue;
    if (!Array.isArray(names)) throw new TypeError(`clock.freeze: ${key} is a list of names`);
    for (const name of names) {
      if (!FAKEABLE.includes(name)) {
        throw new TypeError(
          `clock.freeze: cannot fake ${JSON.stringify(name)}; it fakes ${FAKEABLE.join(", ")}`,
        );
      }
    }
  }
  const loopLimit = options.loopLimit ?? RUNAWAY;
  if (!(Number.isInteger(loopLimit) && loopLimit > 0)) {
    throw new TypeError("clock.freeze: loopLimit is a whole number above 0");
  }
  const wanted = options.toFake
    ? new Set(options.toFake)
    : new Set(
        FAKEABLE.filter((name) => name !== "queueMicrotask" && !options.toNotFake?.includes(name)),
      );
  return { now: options.now, loopLimit, wanted };
}

const clock = {
  /// Stops time. Optionally at a given moment — otherwise wherever it is now —
  /// and, given `{ toFake }` or `{ toNotFake }`, only for some of it. What this
  /// runtime does not have is left alone.
  freeze(at) {
    if (frozen) return clock;
    const { now, loopLimit, wanted } = freezeOptions(at);
    const realDate = globalThis.Date;
    const realPerformanceNow =
      typeof globalThis.performance?.now === "function"
        ? globalThis.performance.now.bind(globalThis.performance)
        : null;
    const state = {
      now: realDate.now(),
      start: 0,
      timers: new Map(),
      jobs: [],
      next: 1,
      loopLimit,
      realDate,
      realPerformanceNow,
      undo: [],
    };
    frozen = state;
    if (now !== undefined) clock.setSystemTime(now);
    state.start = state.now;
    // From a whole millisecond, so the differences a test measures are exact.
    const performanceStart = realPerformanceNow ? Math.floor(realPerformanceNow()) : 0;
    const performanceNow = () => performanceStart + (state.now - state.start);

    // Each replacement is undone by `release`, in reverse.
    const replace = (target, key, value) => {
      const before = Object.getOwnPropertyDescriptor(target, key);
      Object.defineProperty(target, key, {
        value,
        writable: true,
        configurable: true,
        enumerable: before?.enumerable ?? false,
      });
      state.undo.push(() => {
        if (before) Object.defineProperty(target, key, before);
        else delete target[key];
      });
    };
    const fake = (name, value) => {
      if (wanted.has(name) && typeof globalThis[name] === "function") {
        replace(globalThis, name, value);
      }
    };

    const add = (callback, delay, args, every) => {
      const id = state.next++;
      state.timers.set(id, {
        at: state.now + Math.max(0, Number(delay) || 0),
        callback,
        args,
        every,
        id,
      });
      return id;
    };
    const cancel = (id) => void state.timers.delete(Number(id));

    fake("setTimeout", (callback, delay, ...args) => add(callback, delay, args, null));
    fake("clearTimeout", cancel);
    fake("setInterval", (callback, delay, ...args) =>
      add(callback, delay, args, Math.max(1, Number(delay) || 0)),
    );
    fake("clearInterval", cancel);
    fake("setImmediate", (callback, ...args) => add(callback, 0, args, null));
    fake("clearImmediate", cancel);
    // Frames fall every 16ms from the moment the clock froze, and a callback
    // is given the frame's time as `performance.now()` reads it.
    fake("requestAnimationFrame", (callback) =>
      add(
        () => callback(performanceNow()),
        FRAME - ((state.now - state.start) % FRAME),
        [],
        null,
      ),
    );
    fake("cancelAnimationFrame", cancel);
    // Idle as soon as nothing else is waiting; otherwise after 50ms, or the
    // callback's own timeout if that is sooner.
    fake("requestIdleCallback", (callback, options) => {
      const idle = state.timers.size > 0 ? 50 : 0;
      const timeout = Number(options?.timeout);
      const delay = timeout > 0 ? Math.min(timeout, idle) : idle;
      const scheduled = state.now;
      return add(
        () =>
          callback({
            didTimeout: timeout > 0 && state.now - scheduled >= timeout,
            timeRemaining: () => 50,
          }),
        delay,
        [],
        null,
      );
    });
    fake("cancelIdleCallback", cancel);
    fake("queueMicrotask", (callback) => {
      if (typeof callback !== "function") {
        throw new TypeError("queueMicrotask needs a function");
      }
      state.jobs.push(callback);
    });

    // `Date` moves with the clock rather than being frozen separately, because
    // the two are one question: code that waits almost always also asks what
    // time it is, and a stopped `setTimeout` beside a running `Date.now()`
    // describes a machine that does not exist. A function rather than a class,
    // so `Date()` without `new` still returns the time as a string.
    if (wanted.has("Date")) {
      const Date = function Date(...args) {
        if (!new.target) return new realDate(state.now).toString();
        return Reflect.construct(realDate, args.length === 0 ? [state.now] : args, new.target);
      };
      Object.setPrototypeOf(Date, realDate);
      Date.prototype = realDate.prototype;
      Date.now = () => state.now;
      replace(globalThis, "Date", Date);
    }
    if (wanted.has("performance") && realPerformanceNow) {
      replace(globalThis.performance, "now", performanceNow);
    }
    const Temporal = globalThis.Temporal;
    if (wanted.has("Temporal") && Temporal?.Now) {
      const instant = () => Temporal.Instant.fromEpochMilliseconds(state.now);
      const zoned = (zone = Temporal.Now.timeZoneId()) => instant().toZonedDateTimeISO(zone);
      replace(Temporal.Now, "instant", instant);
      replace(Temporal.Now, "zonedDateTimeISO", zoned);
      replace(Temporal.Now, "plainDateTimeISO", (zone) => zoned(zone).toPlainDateTime());
      replace(Temporal.Now, "plainDateISO", (zone) => zoned(zone).toPlainDate());
      replace(Temporal.Now, "plainTimeISO", (zone) => zoned(zone).toPlainTime());
    }
    // A formatter asked to format "now" — no date given — formats the clock's.
    const DateTimeFormat = globalThis.Intl?.DateTimeFormat;
    if (wanted.has("Intl") && DateTimeFormat) {
      const { formatToParts } = DateTimeFormat.prototype;
      const current = (formatter) => {
        const format = formatter.format;
        Object.defineProperties(formatter, {
          format: { value: (date) => format(date === undefined ? state.now : date), configurable: true },
          formatToParts: {
            value: (date) => Reflect.apply(formatToParts, formatter, [date === undefined ? state.now : date]),
            configurable: true,
          },
        });
        return formatter;
      };
      replace(
        globalThis.Intl,
        "DateTimeFormat",
        new Proxy(DateTimeFormat, {
          construct: (target, args, newTarget) => current(Reflect.construct(target, args, newTarget)),
          apply: (target, self, args) => current(Reflect.apply(target, self, args)),
        }),
      );
    }
    return clock;
  },

  /// Starts it again, and puts the real ones back. Timers still waiting are
  /// dropped.
  release() {
    if (!frozen) return clock;
    for (const undo of frozen.undo.reverse()) undo();
    frozen = null;
    return clock;
  },

  isFrozen: () => frozen !== null,

  /// Moves time forward, running whatever comes due on the way.
  advance(ms) {
    const state = ticking("advance");
    const target = state.now + Math.max(0, Number(ms) || 0);
    for (let fired = 0; ; fired += 1) {
      const timer = due(state, target);
      if (!timer) break;
      if (fired >= state.loopLimit) {
        throw new Error(`clock.advance: ${state.loopLimit} timers fired and the queue is not draining`);
      }
      fire(state, timer);
    }
    state.now = target;
    return clock;
  },

  /// `advance`, pausing after each callback so whatever it resolved gets to run.
  ///
  /// This is the one to reach for when the code under test `await`s. The
  /// synchronous form fires every callback with nothing in between, so a
  /// `sleep(10).then(...)` has been *resolved* but its continuation has not run
  /// — and the assertion after it sees the state from before.
  async advanceAsync(ms) {
    const state = ticking("advanceAsync");
    const target = state.now + Math.max(0, Number(ms) || 0);
    // Before looking, not only after: a continuation left over from an earlier
    // advance has not run yet, so the timer it is about to schedule is not in
    // the queue and a loop starting here would decide there was nothing to do.
    await settle();
    for (let fired = 0; ; fired += 1) {
      const timer = due(state, target);
      if (!timer) break;
      if (fired >= state.loopLimit) {
        throw new Error(
          `clock.advanceAsync: ${state.loopLimit} timers fired and the queue is not draining`,
        );
      }
      fire(state, timer);
      await settle();
    }
    state.now = target;
    await settle();
    return clock;
  },

  /// To the next animation frame, running its callbacks and any timer due
  /// before it.
  advanceToNextFrame() {
    const state = ticking("advanceToNextFrame");
    return clock.advance(FRAME - ((state.now - state.start) % FRAME));
  },

  /// Jumps to whenever the next timer is due, and runs it.
  next() {
    const state = ticking("next");
    const timer = due(state, Number.POSITIVE_INFINITY);
    if (timer) fire(state, timer);
    return clock;
  },

  async nextAsync() {
    clock.next();
    await settle();
    return clock;
  },

  /// Runs the queue until it is empty.
  runAll() {
    const state = ticking("runAll");
    for (let fired = 0; state.timers.size > 0; fired += 1) {
      if (fired >= state.loopLimit) {
        throw new Error(`clock.runAll: ${state.loopLimit} timers fired and the queue is not draining`);
      }
      clock.next();
    }
    return clock;
  },

  async runAllAsync() {
    clock.runAll();
    await settle();
    return clock;
  },

  /// Only what is waiting *now* — an interval fires once rather than for ever.
  runPending() {
    const state = ticking("runPending");
    const waiting = [...state.timers.values()]
      .map((timer) => ({ timer, at: timer.at }))
      .sort((a, b) => a.at - b.at || a.timer.id - b.timer.id)
      .map((entry) => entry.timer);
    for (const timer of waiting) {
      if (state.timers.has(timer.id)) fire(state, timer);
    }
    return clock;
  },

  async runPendingAsync() {
    clock.runPending();
    await settle();
    return clock;
  },

  /// The microtasks queued while `queueMicrotask` is faked, and any they queue.
  runMicrotasks() {
    runJobs(ticking("runMicrotasks"));
    return clock;
  },

  /// How many timers are waiting.
  pending: () => (frozen ? frozen.timers.size : 0),

  /// Drops them all without running any, and any faked microtasks.
  clear() {
    if (frozen) {
      frozen.timers.clear();
      frozen.jobs.length = 0;
    }
    return clock;
  },

  /// Where the frozen clock stands. A `Date`, a number of milliseconds, or a
  /// string the platform's `Date` can parse. Timers do not fire because of it.
  setSystemTime(time) {
    const state = ticking("setSystemTime");
    const at = typeof time === "string" ? state.realDate.parse(time) : Number(time);
    if (Number.isNaN(at)) throw new TypeError(`clock: ${String(time)} is not a time`);
    state.now = at;
    return clock;
  },

  /// The real time, while the clock is frozen — for measuring how long
  /// something actually took.
  realNow: () => (frozen ? frozen.realDate.now() : Date.now()),
};

export {
  test,
  it,
  describe,
  suite,
  beforeAll,
  afterAll,
  beforeEach,
  afterEach,
  assert,
  assertEquals,
  assertThrows,
  assertRejects,
  assertSnapshot,
  expect,
  mock,
  clock,
  inject,
  onTestFinished,
  onTestFailed,
  waitFor,
  __setTestFile,
};
export default {
  test,
  it,
  describe,
  suite,
  beforeAll,
  afterAll,
  beforeEach,
  afterEach,
  assert,
  assertEquals,
  assertThrows,
  assertRejects,
  assertSnapshot,
  expect,
  mock,
  clock,
  inject,
  onTestFinished,
  onTestFailed,
  waitFor,
};
