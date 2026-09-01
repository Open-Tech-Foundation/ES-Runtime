# Changelog

All notable changes to ES-Runtime are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/); the project is
pre-`1.0` and the public API (the Rust crates and the `runtime:` standard-module
namespace) is unstable and may change between minor releases until the API freeze
(SPEC §14).

## [Unreleased]

## [0.28.0] - 2026-09-01

### Added

- **`runtime:test` grows the vocabulary a suite written elsewhere expects** —
  `it` and `suite`, `test.each` / `describe.each`, `test.todo`, and
  `test.skipIf` / `test.runIf`.

  ```js
  test.each([
    [1, 1, 2],
    [2, 3, 5],
  ])("adds %d + %d = %d", (a, b, want) => expect(a + b).toBe(want));
  ```

  `%s`/`%d`/`%i`/`%f`/`%j`/`%o` take the next value positionally, `%#` is the
  row's index, `$key` reads a property when the row is an object, and an array
  row is spread into the body's parameters. A name that does not vary per row
  gets its index appended — six cases sharing one identity is a report where a
  failure names none of them.

  `todo`, `skipIf` and `runIf` are all **counted as skipped** rather than left
  out. A case that vanishes from the tally is the failure this runner is
  arranged against.

- **`esdev test --setup`, `--timeout` and `--reporter`**, each also an
  `esdev.json` key under `"test"` — with `jobs` — because a project's setup
  files and its per-file budget are properties of the project. A flag beats the
  file.

  ```json
  { "test": { "setup": ["./test/setup.ts"], "timeout": 5000,
              "jobs": 4, "reporter": "json" } }
  ```

  A **setup module is imported before the file under test**, so a global it
  stubs is in place before anything reads it — and it costs no line numbers,
  which is the property D71 is built on. (The first version prepended it to the
  *source*, which goes through the printer, and every frame in every failure
  pointed one line off.)

  **`--timeout` ends the process.** The case it exists for is a file that
  wedges, and a budget the file kept for itself is one it never gets round to
  noticing. There is no default: a suite is not a place to guess how long a
  machine takes.

  **`--reporter=json`** writes one object per line — a `case` per failure, a
  `file` per file, a `summary` at the end — so a CI job reads results as they
  land instead of parsing a document once the run is over.

- **A library is describable in `esdev.json`** — `"lib"`, `"format"`, `"types"`
  and `"dts-bundle"` are target keys, matching the flags of the same names.

  ```json
  {
    "targets": {
      "lib": {
        "entry": "src",
        "lib": true,
        "format": ["esm", "cjs"],
        "outdir": "dist",
        "minify": true,
        "sourcemap": true,
        "assets": ["README.md", "LICENSE"]
      }
    }
  }
  ```

  They were flags only, and that was a hole rather than an omission: everything
  else about a build lives in that file, so a library describable only on a
  command line could describe only *part* of itself. `assets` is a target key —
  the README and LICENSE a package ships — so a `--lib` build had no way to name
  them, **and the `--lib` path never copied them either**. Both halves are fixed.

  A library target is refused where the flags are refused and in the same words:
  `format`/`types`/`dts-bundle` off a library, a library writing one `out` file,
  a library rooted at a document, a library with `"then": "run"`. And
  `"dts-bundle": true` resolves its entry when the file is *read*, so a missing
  `index.ts` names what was looked for rather than failing later from inside a
  build.

  The rule, for what comes next: an option that shapes a build belongs on both
  surfaces. The two disagreeing is how a project gets built one way from a
  script and another from the file.

- **`expect`, `mock` and `clock` in `runtime:test`** — the assertion vocabulary
  the ecosystem writes tests in, plus the two subsystems that stand in for
  something real.

  ```js
  import { test, expect, mock, clock } from "runtime:test";

  test("polls every second until it answers", async () => {
    clock.freeze();
    const ask = mock.fn().mockResolvedValueOnce(null).mockResolvedValue("ok");

    const answer = poll(ask, 1000);
    await clock.advanceAsync(2000);

    expect(ask).toHaveBeenCalledTimes(2);
    await expect(answer).resolves.toBe("ok");
    clock.release();
  });
  ```

  `expect` is a **second spelling, not a second implementation** —
  `assertEquals(a, b)` and `expect(a).toEqual(b)` share one comparison — with
  `.not`, `.resolves`/`.rejects`, the asymmetric matchers (`expect.any`,
  `expect.objectContaining`, …) and the call matchers that read a mock's record.

  `mock.fn()` records `calls`, `results`, `instances` and `lastCall` and answers
  however it was told to; `mock.spyOn(o, k)` installs one over a real method and
  **still calls the original**, because a spy is usually installed to watch
  something work. `clock.freeze()` replaces `setTimeout`, `setInterval`, their
  cancels and `Date`, so code that waits can be tested without waiting.

  `clock.advanceAsync(ms)` is the one to reach for when the code under test
  `await`s: the synchronous form fires every callback with nothing in between,
  so a `sleep(10).then(…)` has been resolved but its continuation has not run.

  **Still imported, never ambient.** There is no global `test` and no global
  `expect`, here or anywhere — the guarantee this project's API reference opens
  with has no exception for test files. A suite written for another runner gets
  an import line, which is a line. Freezing the clock does replace globals, but
  they are standards-defined names replaced at the test's own explicit request,
  and a test file is a process, so the swap cannot reach the next file.

  Named `mock` and `clock` rather than borrowing another runner's namespace.

- **`esdev` resolves imports the way `esdev build` does** — `./util` finds
  `util.ts`, a directory finds its `index.*`, and `./util.js` finds the
  `util.ts` TypeScript tells you to spell that way.

  The two halves of `esdev` disagreed: `esdev build src/app.ts` bundled a source
  tree written for a build step without complaint, while `esdev src/app.ts`
  refused to run the same tree. Most published TypeScript is written that way,
  so the runner could not be pointed at it at all.

  `esrun` is **unchanged** and still resolves only what the module spec says: a
  production binary that guessed at filenames would be reading the disk to
  decide what a program means. What you deploy has been through a build, which
  resolved all of it — so ship the build, not the source. A miss still reports
  the specifier the file wrote, not the last spelling tried.

- **`esdev test` discovers `*.spec.*` as well as `*.test.*`** — both conventions
  are everywhere, and a runner that knows only one silently runs no tests in
  half the projects it is pointed at, which looks exactly like a suite that
  passes.

- **`runtime:test` groups its tests, and can skip or focus one** — `describe`,
  `test.skip`, `test.only`, and `.skip`/`.only` on a group.

  ```js
  describe("db", () => {
    beforeAll(() => open());       // once, before this group's first test
    beforeEach(() => reset());     // around this group's tests, and no others

    test("inserts", async () => …);
    test.only("the one being worked on", async () => …);
  });
  ```

  A group composes the name — `"db > constraints > rejects a null"` — and, the
  half that earns it, **scopes the hooks**. Without that, `describe` is a naming
  convention, which a template string already is, and a file that sets up a
  database for six of its twenty cases would still be setting it up for the
  other fourteen. `beforeEach` runs outermost-first and `afterEach`
  innermost-first; a group's `afterAll` runs when *its* last case has run, not
  at the end of the file.

  A skipped case is **counted in the tally**, not left out of it — the same
  reasoning as a case that never finished being a failure rather than a silence.
  `only` says how much it held back, on a line of its own, because a `.only` left
  in a commit otherwise looks exactly like a suite that got faster:

  ```text
    only: 27 other tests did not run
    1 passed, 0 failed, 27 skipped
  ```

  A `describe` body registers and returns; an `async` one is refused rather than
  half-run, since only the part before its first `await` would register in time.

- **`runtime:fs` can make a symbolic link**, not only read one. `readLink` and
  `realPath` have been there since the module shipped, and there was nothing
  that created what they read.

  ```js
  import { symlink } from "runtime:fs";

  await symlink("../shared/pkg", "node_modules/pkg");
  ```

  **The target is data, not a path being accessed.** It is stored verbatim, is
  what `readLink` hands back, and may be relative, may not exist, and may name
  somewhere outside the root jail — which is what makes "a dependency that
  resolves outside the project" reproducible in a test without shelling out to
  `ln -sfn`. It is not a hole: only the link's own location is jailed, as a
  write, and every read *through* the link goes back through the same
  canonicalize-then-confine as any other path. So a link out of the jail is one
  the program that made it cannot follow.

  `ERR_ALREADY_EXISTS` if the path is taken, like Node's and Deno's. `type:
  "file" | "dir"` is the Windows question of which kind of link to create, and
  defaults to `"file"` as Node's does — deliberately **not** inferred by looking
  at the target, because the target is unjailed data and a lookup there would be
  a metadata read at an arbitrary path.

  Needs `FileWrite` alone: creating a link stores a string and reads nothing.

- **`esdev build --lib --format=cjs` publishes to consumers who are not on this
  runtime.** The runtime loads ES modules only and that has not moved — this is
  an *output*, for the Node programs that will `require()` a package built here.

  ```sh
  esdev build --lib src --format=esm,cjs   # dist/**.js + .d.ts, dist/**.cjs + .d.cts
  esdev build --lib src --format=cjs       # CommonJS alone
  ```

  Both trees go in one directory, told apart by extension rather than by the
  package's `"type"` field, so flipping that field cannot change which files an
  `exports` map has to name. A `.cjs` is typed by the `.d.cts` beside it and by
  nothing else under `node16` resolution, so one is emitted for each. `types`
  belongs inside each condition:

  ```json
  "exports": {
    ".": {
      "import":  { "types": "./dist/index.d.ts",  "default": "./dist/index.js" },
      "require": { "types": "./dist/index.d.cts", "default": "./dist/index.cjs" }
    }
  }
  ```

  One pass per format, and none of them lands unless every one succeeds — a
  failure names the pass it happened in. A top-level `await` has no CommonJS
  form and stops the build; that module publishes as ESM only. A `runtime:`
  import stays external in both trees and nothing outside `esrun` can resolve
  the `require` it becomes, so the build says when it wrote one.

  `--format` is `--lib` only: an application's output is loaded by `esrun`.

- **A `--lib` build's declarations name the files it wrote.** They used to
  repeat the specifier the source wrote while the JavaScript beside them named
  the emitted file — so a library whose source imports a sibling as `./pool`,
  which is how a source written for a bundler spells it, shipped declarations a
  `node16` consumer rejects (TS2835), and a `.d.cts` importing `./pool.js`
  resolved to the ES module's declarations and could not be required (TS1479).
  Both halves are now rewritten to their own format's sibling — `./pool.js`
  beside a `.js`, `./pool.cjs` beside a `.cjs` — including the `import("./x")`
  types that can sit anywhere in a type. An extensionless specifier is resolved
  against the source tree rather than guessed at, since `./pool` is either
  `pool.ts` or `pool/index.ts`.

  Found by building `@opentf/std` with it: 334 modules, 717 `tsc` errors under
  `node16` with `skipLibCheck` off, and 0 after. Its 1642-test suite passes
  against the built ES modules and against the CommonJS output loaded through
  `require()`.

- **`--dts-bundle` keeps `declare` on a default export, and refuses an
  `import("./x")` type instead of dangling it.** `export default function
  f(): string;` carries no `declare` — the export modifier is what made it a
  declaration — so inlining it as `function f(): string;` was TS1046 in the
  bundled file. Every library whose modules default-export was affected;
  `@opentf/std` had 317 of them in one `index.d.ts`.

  An inline `import("./x")` type is a reference to another module of the
  library, and linking resolves import *statements* — so it survived into the
  single file with nothing beside it to resolve to (TS2307). It now stops the
  build and names itself, like every other construct this linker cannot link.
  An `import("a-package")` is untouched: a package is the consumer's to resolve.

  A module is also named in diagnostics as it is spelled — `src/datetime/types.ts`,
  not the `src/./datetime/./types.ts` that joining a specifier onto a directory
  produces.

- **Source maps, and both halves of them.** `esdev build --sourcemap` writes a
  `.map` (or `=inline`, or `=hidden`); `"sourcemap"` says the same per target in
  `esdev.json`. Off unless asked for a release build — a map beside a deployment
  costs bytes and discloses source — and **on in the dev loop**, where neither
  half applies.

  The half that was missing everywhere: **`esrun` reads them when it prints a
  stack trace.**

  ```text
  Error: too big
      at boom (file:///srv/app/src/util.ts:2:14)      # not dist/server.js:3:19
  ```

  Only the printed frames change; `error.stack` inside the program stays what
  the engine built, so a program shipping its own stack to a reporter still
  sends the truth about the file that ran. A map's `sources` are absolute,
  which is what makes this work at all: a build stages its output and moves it
  into place, so a path relative to the map named a directory that no longer
  existed a moment later.

- **`import logo from "./logo.png"`.** It used to stop the build — the bundler
  read the image as source and reported that it was not valid UTF-8. The file is
  now emitted with a content hash and the import becomes its URL,
  `/assets/logo-1a2b3c4d.png`.

  The `assets` copy could not answer this: it cannot hash a name, so a changed
  image keeps its URL and a cache serves the old one; and it does not know the
  file was referenced at all, so forgetting to list one is a 404 in production
  rather than anything the build says. CSS already disagreed with it —
  `url(./logo.png)` in a stylesheet has always been followed and hashed. Images,
  fonts, media, PDFs and `.wasm`; `.json` stays a module. The URL is rooted and
  identical in the server and browser bundles, so the markup one renders names
  the file the other fetches. `--lib` refuses it, naming the reason: a library
  cannot know where the consuming build serves a file from.

  `assets` remains the answer for a file nothing imports, where the name must
  *not* change.

- **A plugin's own message survives a load failure.** The bundler renders one as
  "plugin `x` threw an error" and keeps only that line, so what the plugin
  actually said — including every guest plugin's — was dropped from the
  diagnostic. It is printed under the frame now.

- **`alias`: a specifier rewrite the build is told about.** `@/db` is how most
  source is written and was an unresolved import here.

  ```json
  { "alias": { "@": "./src", "react": "preact/compat" } }
  ```

  Top level in `esdev.json`, because a rewrite is a property of the source tree
  rather than of one output; `--alias=@=./src` says the same for an entry with
  no project around it. A path resolves against the project, anything else names
  a package, and the longest match wins. It is a **bundling** rule: a module run
  unbundled — `esdev src/thing.ts`, or a file `esdev test` runs — resolves the
  way `esrun` does. `--lib` refuses it, since a published module keeps the
  specifier its source wrote.

- **`import.meta.env`, from `.env` and the environment.** A browser bundle
  cannot read the environment at run time, so what configures it is compiled in
  — and only what says out loud that it may be:

  ```sh
  PUBLIC_API_URL=https://api.example.com    # .env, or the environment itself
  ```

  ```ts
  fetch(`${import.meta.env.PUBLIC_API_URL}/users`);
  if (import.meta.env.DEV) { … }
  ```

  `PUBLIC_`-prefixed variables only; everything else stays out of the artifact
  and is read at run time through `runtime:env`, under the `env` capability.
  `.env` first, then the environment, so a variable exported in the shell beats
  the file. `MODE`, `DEV` and `PROD` come from the build — booleans, not the
  strings `"false"` — and `import.meta.env` itself is replaced too, so
  destructuring it works. A `--lib` build compiles in none of it.

- **`esdev preview` — serve the release build before deploying it.**

  ```sh
  esdev build && esdev preview        # → http://127.0.0.1:4173
  ```

  The dev loop's build is not the one that ships (`NODE_ENV` is
  `"development"`, nothing is hashed), so the failures that only appear in a
  release build had nowhere to appear before it was deployed. It serves and does
  not build; missing paths that look like routes fall back to `index.html`;
  loopback only. A project whose output is a server bundle is told to run it
  under `esrun`, which is what production will do.

- **`esdev test` runs files in parallel, and can watch.** One process per file
  is what makes the runner robust, and it was also what made it slow: they ran
  one at a time. The default is now the machine's parallelism, at most 8 — every
  job holds a V8 heap — and each file's output is held and printed whole, so two
  suites never interleave line by line.

  ```sh
  esdev test                  # all of them, in parallel
  esdev test db --watch       # ...and again on every save
  esdev test --jobs=1         # one at a time, writing straight through
  ```

  `--watch` re-discovers files every pass, so the test you are about to write is
  one it will find. `--file` refuses both, being one run of one file.

- **`runtime:build` takes an `exports` output option** (`"auto"` | `"named"` |
  `"default"` | `"none"`) — how a non-ESM `format` assigns what a module
  exports. The same option `--lib`'s CommonJS output sets to `"named"`.

### Fixed

- **`runtime:watch`'s `add()` no longer watches one tree twice.** It refused a
  path already in the set, comparing for equality — so the exact repeat was
  caught and the overlap was not. A dev server that watches `app/` and then adds
  the package `app/` lives in as a dependency was watching that subtree twice,
  which is every event delivered twice on the backends that allow it. A
  recursive watch now covers what is inside it, in both directions: a path
  inside an existing watch is refused, and one that encloses existing watches
  replaces them.

- **An HTML entry no longer builds an empty page and succeeds.**
  `src="/src/main.jsx"` — what Vite's own React template ships, so what people
  arrive with — built 0 scripts and exited 0: a rooted path is a URL the
  deployment already serves, so the reference was left alone and the entry it
  named was never built.

  A rooted reference that names a **source file of this project** is refused
  now, with both spellings in the message. What makes it decidable rather than a
  guess is the target's `assets` list: a rooted URL the assets copy will satisfy
  — `/styles.css` with `"assets": ["styles.css"]`, or a `public/` directory
  holding it — is a correct spelling and stays one. What is left is a rooted URL
  naming a file nothing copies to the output, which is a 404 in production
  whatever this build thinks of it.

- **A plugin's error says which module it happened in.** `ctx.error()` from a
  `transform` produced a diagnostic with `id: null` and `plugin: null` — the
  hook had been called *with* the id and the message named the plugin — whose
  `message` was a JS stack through `runtime:build`'s own dispatcher wrapped in
  the bundler's `plugin \`x\` threw an error / Caused by:` chain, and whose
  `frame` was that same text again behind a `[x]` banner rather than a code
  frame.

  ```js
  ctx.error("cannot compile this route");
  // → { id: "app/page.jsx", plugin: "otfw", message: "cannot compile this route",
  //     frame: null }
  ```

  The bundler carries neither the plugin nor the module: a hook fails with an
  error, and *which module a hook was called about* is not something it tells a
  plugin driver at all. Both are attached where they are known — the one place
  that called the hook — and read back off when the batch is. A plugin that
  **crashed** rather than reported keeps its stack, because the first frame of
  that one is the line in the plugin; `ctx.error()` does not, because the first
  frame of that one is the plumbing. The terminal line names the plugin:
  `[otfw] app/page.jsx: cannot compile this route`.

- **Breaking (plugins written against 0.4): a plugin filter that names nothing
  is refused, instead of claiming the whole graph.** `filter` is read for `id`
  and `code`; anything else in it used to
  produce an *empty* filter, and an empty filter is not "no modules" — it is
  every module, because a hook with no filter is a hook that wants the graph.

  ```js
  transform: { filter: /\.mdx$/, handler }        // ✗ refused, names no field
  transform: { filter: { id: /\.mdx$/ }, handler } // ✓
  ```

  The bare pattern is the spelling this API had before 0.5, so **every plugin
  written against the old one became a silent catch-all on upgrade** — running
  on modules it was never written for, and rewriting them. The same went for a
  typo'd `ID`, for rollup's `include`, for `{}`, and for `{ id: [] }`. All five
  are now refused at the declaration, where the person who wrote them is
  looking, and the message names the field that is missing.

### Security

- **The filesystem jail acts by descriptor, not by name** (DECISIONS D83). Every
  jailed operation used to look the same name up twice: `confine()` resolved the
  path and proved it sat under a root, then the syscall resolved that *string*
  again. Between the two the filesystem is mutable, so a guest running two
  operations at once could replace a directory component with a symlink after
  the check and before the use — and the write followed the new one, out of the
  jail, with the jail reporting success.

  The window was not small: resolution happens eagerly, before the future is
  constructed, and the syscall lands a poll and a blocking-pool handoff later.
  And it stopped being theoretical when `symlink()` arrived in this release —
  creating the link previously needed `ln`, and so `RunProcess`, or a process
  outside the jail; it now needs `FileWrite` and nothing else.

  Resolution ends by **opening the parent directory**, and the operation runs
  relative to that descriptor. A descriptor refers to an inode rather than a
  name, so nothing done to the name afterwards can redirect it. Each step below
  the root is opened `NOFOLLOW`, so a component that has become a symlink since
  the check is refused as an escape rather than followed; recursive `mkdir` and
  `remove` descend the same way, one descriptor at a time, because a recursive
  delete redirected halfway is the one outcome that cannot be undone.

  Covered: `read`, `write`, `mkdir`, `remove`, `rename`, `symlink`, `readLink`,
  `truncate`, `chmod`. A link that was **already there** is still followed by
  the operations that always followed it — `read`, `write`, `truncate` and
  `chmod` resolve it through the jail, so a symlinked config file is written by
  writing it. What is refused is a link **swapped in after** the check, which is
  a different thing.

  **Breaking (`remove` and `rename` on a symlink):** those two are about the
  link, and they resolved it before — so `remove("link")` deleted the file at
  the other end of it and `rename` moved that file rather than the link. They
  act on the link itself now, as every other runtime does.

  Not covered, and written down rather than implied:
  **Windows**, which has no `*at` family and keeps the old behaviour; **hard
  links**, which `NOFOLLOW` cannot see and which is why there is no `link()`
  operation; and **timing side-channels**, which no filesystem can close.

  No new dependency — `rustix` was already direct here, chosen because this
  crate is `forbid(unsafe_code)`.

## [0.27.0] - 2026-08-19

### Added

- **`runtime:workers` — durable workers.** State that outlives the process, in
  `esrun`'s own SQLite, with no service to run beside it (DECISIONS D80).

  ```js
  import { DurableWorker } from "runtime:workers";

  export class Cart extends DurableWorker {
    async add(item) {
      const items = this.state.get("items") ?? [];
      items.push(item);
      this.state.set("items", items);
      return items.length;        // held back until that write commits
    }
  }

  await Cart.get("u_42").add({ sku: "A1" });
  ```

  A durable worker is **addressed, not spawned**: name one and the runtime opens
  it, runs one call at a time against it, closes it when it goes idle, and finds
  its state where it was left the next time anyone names it.

  - **A call's result is not delivered until the writes it made have
    committed.** A crash is then a call that never returned, never one that
    returned a lie — and that gate is what makes coalescing the writes safe.
    Held to by a test that `SIGKILL`s a real process after five acknowledged
    appends and finds five.
  - **Reads are synchronous.** The key/value state is resident, so
    `state.get(k)` is a map lookup rather than an await. Which is only sound
    with a real ceiling, so there is one: 1 MiB a worker and 128 KiB a value,
    refused at the write.
  - **Anything `structuredClone` carries can be stored** — `Date`, `Map`,
    `Set`, typed arrays, `BigInt`, cycles — not only what JSON survives.
  - **One process per directory**, enforced by the engine's own exclusive lock
    on the state files, so nothing goes stale when a process is killed. A
    second process is refused by name (`ERR_DURABLE_LOCKED`) until the first
    exits.
  - **No new capability and no new Rust.** State is files: `--allow-read` and
    `--allow-write`, under the same jail everything else uses. The module is
    guest JavaScript over `runtime:db`, `runtime:fs` and `runtime:hashing`.

  Shards (a worker on a `Worker` of its own) are the phase after this one, and
  absent rather than present as an option that does nothing.

- **Collections: durable-worker state that grows** (DECISIONS D82). The keys are
  resident and capped; a collection is documents in a table of their own,
  queried rather than held.

  ```js
  export class Room extends DurableWorker {
    static schema = {
      collections: { messages: { index: ["ts", "author"], unique: ["clientId"] } },
    };

    async recent(n = 20) {
      return this.state.collection("messages")
        .find({ ts: { gte: Date.now() - 86_400_000 } })
        .sort({ ts: "desc" })
        .limit(n)
        .toArray();
    }
  }
  ```

  - A document is stored the way a key is — structured clone — so a `Date` comes
    back a `Date`. What the class **declares** is copied into a real indexed
    column beside it, and that is what can be matched and sorted; a name or a
    field the class did not declare throws, unless the query asks for
    `{ scan: true }`.
  - `insert`, `insertMany`, `get`, `update`, `delete`, `deleteWhere`, `count`,
    and a `find` builder with `sort` / `limit` / `offset` / `toArray` / `first`
    / `count` / `for await`.
  - `state.transaction(fn)` covers the keys and the collections together.
  - **A field declared later gets its column and a backfill** on the first wake
    after the deploy, so a query over it does not quietly miss what came before
    it. Nothing is ever dropped.

- **Durable workers can ask to be woken** (DECISIONS D81). `state.alarm.set(when)`
  stores a time beside the worker's state, so it survives a restart — and the
  worker is woken whether or not anybody addresses it.

  ```js
  export class Reminder extends DurableWorker {
    async schedule(at) { await this.state.alarm.set(at); }
    async alarm() {
      await deliver(this.state.get("message"));
      // setting the next one here is how a worker repeats
    }
  }

  startAlarms({ classes: [Reminder] });   // a process says it runs alarms
  ```

  - The alarm is **cleared before the handler runs**, and `alarm()` goes through
    the same mailbox a call does — so it never interleaves with one, and its
    writes are gated the same way.
  - **A failing `alarm()` is retried** (1s, 2s, 4s… to a five-minute cap,
    `alarmRetries` times) with the count stored, then cleared and **reported**:
    a scheduled job that fails silently is how a queue loses work.
  - `startAlarms({ classes })` requires the list. Anything scheduled for a class
    a process does not list is left for the process that does, rather than
    firing on whichever deployment happened to be busy.
  - While the scheduler runs, the process stays alive — which is why it is not
    started for you: a script that scheduled something for tomorrow should not
    sit there until tomorrow.


- **`runtime:process` — the process's own `stdout` and `stderr`.** `console.log`
  formats a value, appends a newline and goes wherever the embedder pointed it.
  That is right for a log line and wrong for a **display**: a spinner is a
  carriage return and no newline, and a progress bar rewrites the line it is
  already on.

  ```js
  import { stdout } from "runtime:process";

  if (stdout.isTTY) {
    stdout.write(`\r${bar(done / total, stdout.columns ?? 60)}`);
  } else {
    console.log(`${done}/${total}`);
  }
  ```

  `write(chunk)` puts exactly those bytes on the stream and flushes — a string
  is UTF-8, an `ArrayBuffer` or view goes as it is, and **no newline is added**.
  `isTTY` says whether anyone is looking: a spinner redrawn with `\r` into a log
  file is a file of spinner frames, and colour escapes in a pipe are noise in
  somebody's `grep`. `columns` and `rows` come from the terminal itself rather
  than from `$COLUMNS` — a shell exports that to itself rather than to a child,
  and it is stale the moment the window is dragged — and are `undefined` where
  the host cannot answer, rather than a plausible 80.

  **No capability**, for the reason `console.log` needs none: writing to the
  stream this program was started with reaches nothing it was not already
  handed. So `--deny-all` still leaves a program able to say what it is doing.

  The `Process` provider trait gains `write_stdout`, `write_stderr`,
  `is_terminal` and `terminal_size`, all defaulted — an existing embedder
  compiles unchanged and reports "no standard output, not a terminal" until it
  implements them.

### Fixed

- **A child's output is read on demand, not one chunk ahead.** A program that
  took the one chunk it wanted from a child's `stdout`, printed its summary and
  returned never exited — `timeout 5` and exit 124.

  A `ReadableStream` with the default high-water mark reads one chunk *ahead*,
  so a `system_read` was left outstanding on a child that had nothing more to
  say, and pending host work is what keeps the runtime ticking. Both of a
  child's output streams are strictly demand-driven now: a read is in flight
  only because somebody is waiting for it, which is also the honest queue depth
  for a pipe, whose backpressure is the pipe itself.

- **A package's stylesheet is not CommonJS.**
  `import.meta.resolve("tailwindcss/index.css")` failed with *"this package is
  CommonJS, which is not supported"*. It is not: it is a stylesheet.

  The check asked whether the resolved file was ESM and answered "no" for every
  extension it did not recognise, so a `.css`, `.json` or `.wasm` published in
  an `exports` map came back as a CommonJS rejection — and CSS inside a package
  needed a hand-written package resolver to find. Resolution says where a
  subpath lives; what the caller does with the file is the caller's business.
  Only `.cjs`, and `.js` outside a `"type": "module"` package, are CommonJS now.
  The rejection those get is unchanged (D24).

## [0.26.0] - 2026-08-18

### Fixed

- **An npm-installed program can find its dependencies.** Running a file inside
  `node_modules` — the entry point of any installed CLI — anchored the project
  root at the *package's* own `package.json`, so the `node_modules` walk stopped
  inside the dependency and every hoisted package was unreachable
  (`cannot find package "leftpad" … no node_modules/leftpad under the project
  root <proj>/node_modules/@acme/cli`). `cd <proj> && esrun
  node_modules/@acme/cli/src/cli.js` now resolves what a package manager hoisted
  beside it (DECISIONS D79).

### Changed

- **The sandbox is the directory you run in.** The project root — the
  `node_modules` walk's ceiling and the filesystem jail, one directory for both
  — was detected by walking up from the entry file. It is now **the working
  directory, exactly**: no walk, no marker file, no flag.

  An entry is a path someone typed, so a root derived from one moves when the
  argument moves; and a root *detected* by walking is one a stray `package.json`
  two directories up can silently widen. Neither is a boundary. The working
  directory is the one anchor guest code cannot reach, no argument can shift,
  and an operator can read off their shell prompt.

  A missing `package.json` is **not** an error — an image with `dist/` and
  `node_modules/` and no manifest is an ordinary deployment, and the jail is
  that directory either way.

- **`esrun` refuses to run in a filesystem root or your home directory.** These
  are the two working directories that are enormous by nature: `/` is what an
  image with no `WORKDIR` and a systemd unit with no `WorkingDirectory=` give
  you, and `$HOME` is where cron starts. Anchoring the jail there would put
  every file on the machine, or every credential you own, inside it — so the run
  stops at startup and names the fix instead.

- **An entry outside the working directory is refused**, with a message naming
  the root, rather than starting and then reporting every import as escaping a
  jail. A program is run from its own directory: `esrun /srv/app/server.js` from
  `/` is now an error.

### Changed

- **`esrun --help` is 62 lines instead of 93.** What it kept is the grammar,
  the permission vocabulary, the flags and the four lines that show how a grant
  is widened; what it lost is the prose the site holds in one place —
  scope-matching semantics, the import-policy format, the module-resolution
  rules — each now a URL at the foot of the help. `esrun upgrade`'s
  implementation moved to `cli-common` unchanged in behaviour, so that `esdev`
  can upgrade itself through the same code (DECISIONS D77); which release each
  binary resolves is still decided by its own tag prefix.


## [0.25.0] - 2026-08-15

### Added

- **Embedders can add `runtime:` modules of their own.**
  `Runtime::register_module(specifier, source)` serves a module on the same
  terms as a baked one — no loader, no filesystem, and no capability to import
  it, because the gate is always the op. Shadowing a built-in is refused; a
  specifier outside the `runtime:` scheme is refused. An op registered after the
  snapshot was taken gets its JS shell like any baked one
  (`Engine::finish_baked_ops`), so a module added this way is indistinguishable
  from one that shipped in the binary.

  This is the seam `esdev` 0.2.0's `runtime:build`, `runtime:test` and
  `runtime:watch` arrive through, and the reason `esrun` does not merely leave
  those unwired but does not contain them: importing one under `esrun` fails at
  load with *unknown built-in module*. See
  [`crates/dev-cli/CHANGELOG.md`](crates/dev-cli/CHANGELOG.md) for what `esdev`
  gained.

### Changed

- **The serialization suite runs on `esdev test`**, not `bun test` — the
  runtime's largest hand-written JS subsystem now gates on the binary we ship,
  exercising its module loader, type stripping and event loop. Its `.js`
  import specifiers moved to `.ts`, matching every `esdev` template and the
  runtime's rule that a specifier names a file that exists (D21/D40).

- **The repository is one Bun workspace.** The root `package.json` declares
  `workspaces` and `packageManager`; `packages/*` and `crates/runtime/js` are
  members with a single lockfile between them.

  This was a CI bug, not only tidiness: `tsr install` runs one root
  `bun install`, and without a workspace that installed Biome and nothing else
  — so `tsr typecheck` and `tsr build` ran in packages whose `typescript` had
  never been installed. It passed locally only because those directories had
  been installed by hand.

- **The home page's architecture diagram is now an animated security diagram.**
  The old static "Simple Runtime Architecture" row is replaced by an inline SVG
  that draws the deny-by-default model: guest code on the left, a capability
  gate in the middle, host resources on the right, and the nine capability
  names below. On a 7s loop a `fetch` op reaches the gate without `net`, is
  refused (`✕ ERR_CAPABILITY_DENIED` — the error thrown *before* the effect),
  and bounces back; a `fs.read` op passes through to the file system. The
  animation is pure CSS (`global.css`), confined to the shield via a
  `clip-path`, and honours `prefers-reduced-motion`.

- **The JavaScript in this repo builds with our own `esdev`.** The two database
  drivers build with `esdev build --lib src`, and the `runtime:serialization`
  bundle with `esdev build` — replacing a `tsc` shim and `Bun.build`
  respectively. The tool that produces the module embedded in the runtime is now
  the one we ship.

  `esdev build --lib` output is **byte-identical to what `tsc` emitted** for
  `packages/postgres` — all 16 files. The serialization bundle exports the same
  names and passes the full conformance suite (374/374).

- **`tsr` runs the tasks** (`tasks.toml`): `tsr build`, `tsr typecheck`,
  `tsr test:unit`, `tsr lint`, `tsr format`, `tsr ci`. Each package keeps its own
  scripts — what a package builds is a property of that package — and tsr is what
  runs them together.

- **Biome lints and formats the JS packages** (`biome.json`), pinned at the repo
  root. Rules that fought deliberate idioms here are off with their reasons
  recorded: `!` answers `noUncheckedIndexedAccess` in the protocol parsers,
  `while ((n = readVarint()))` is how a wire decoder is written, and `msg["S"]`
  names a wire field.

- **CI is split three ways**, so a change pays only for what it can break:
  `ci.yml` (fmt, clippy, the test matrix, Miri) on every PR; `js.yml` (drivers,
  serialization) and `hardening.yml` (MSRV, fuzz, cargo-deny, cargo-audit) only
  when the relevant files changed — plus always on main, and daily for the
  audits, which go stale on their own. A Rust-only pull request drops from
  **eight V8-linked builds to four**.

  Both companions always *trigger* and skip at the job level: a required check
  that never arrives blocks a merge forever, while a skipped job reports success.

- **The project's goal is narrowed to a secure server runtime, and the embedding
  API is no longer offered** (DECISIONS D66). This repo opened as "Layer A", an
  embeddable runtime built so a future actor-model VM ("Layer B") could embed
  it. The binary became the product instead — an HTTP/2 and WebSocket server, a
  database module, a build system, a scaffolder, a deny-by-default deploy line —
  while the embedding API never shipped and nobody asked for it.

  **Nothing is withdrawn.** No crate here was ever published to crates.io, so
  there is no consumer to break; what changes is what the documentation claims
  is on offer. The actor-model VM is a separate project with different goals,
  and the non-goal it implies — no process model, scheduler, preemption,
  mailboxes or supervisors — is unchanged.

  **The architecture is unchanged.** The provider seam, the driven loop that
  owns no thread, the engine abstraction that names no V8 type and the
  capability model all stay: they were justified by testability, determinism and
  a security boundary in Rust rather than in JavaScript, and those hold
  regardless.

  `/docs/embed` and the site's "Embeddable Engine" call-to-action are gone, and
  "Layer A" / "Layer B" is removed from `README.md`, `SECURITY.md`,
  `ARCHITECTURE.md`, `SPEC.md`, `SECURITY-REVIEW.md`, `Cargo.toml` and five
  crate rustdocs. Locked `DECISIONS.md` entries keep their original wording —
  an ADR that edits its own history is not a record.

- **The installer places both binaries**, into `~/.es-runtime/bin`:

  ```sh
  curl -fsSL .../install.sh | bash                      # esrun + esdev
  curl -fsSL .../install.sh | bash -s -- --only=esrun   # servers, CI
  ```

  On Windows, `irm | iex` cannot take arguments, so the selector is
  `$env:ES_RUNTIME_ONLY`. Versions pin independently with `ESRUN_VERSION` /
  `ESDEV_VERSION`, each accepting a bare `0.24.0`, a full `esrun@0.24.0`, or a
  legacy `v0.23.0`. The prefix moved from `ESRUN_INSTALL` to
  `ES_RUNTIME_INSTALL`; the old name is still honoured.

  **The install directory moved** from `~/.esrun/bin`, which only ever named one
  of the two binaries. The old directory is left in place — the installer does
  not delete binaries it did not put there — and is reported when found, because
  a stale `esrun` earlier in `PATH` would shadow the new one. Remove `~/.esrun`
  and its `PATH` entry.

  On Windows ARM64 the installer now says that `esrun` has no release asset for
  that platform (`esdev` does) and installs what it can, rather than failing
  with a bare download error.

- **BREAKING: `esrun` grants nothing by default.** `esrun app.js` used to hold
  every capability; it now holds none, and a run reaches what the command line
  that started it named (DECISIONS D65, superseding D38's default).

  ```sh
  esrun app.js                                # computes, reaches nothing
  esrun --allow-imports --allow-listen=8080 server.js
  esrun --allow-all app.js                    # what the old default was
  ```

  `esrun` stopped being a script runner some releases ago and is now the binary
  a **service** is deployed with, and a service's grant is not a preference — it
  is what an auditor reads. A default of "everything" makes the safe
  configuration the one you have to remember.

  Migrating: `--allow-all` (or `-A`) is the one-flag escape hatch, and
  `esdev --trace-permissions app.js` runs the program and prints the narrow
  `esrun` line it actually needs. **A command line that was already narrow is
  unaffected** — `--deny-all` is still accepted, still means "nothing granted",
  and is still worth writing on a deploy line so a reader need not know which
  way a binary defaults.

  The vocabulary, the scope lists, `permissions.has()` and every capability
  check are unchanged. What moved is the baseline, and with it the direction
  each mode runs in:

  | Mode | Baseline | Direction |
  | ---- | -------- | --------- |
  | `--allow-<name>` | nothing granted (**the default**) | additive only |
  | `--allow-all --deny-<name>` | everything granted | subtractive only |

  `--deny-<name>` now requires `--allow-all`, exactly as `--allow-<name>` used
  to require `--deny-all`. D38's property is intact: no flag ever overrides
  another, so there is still no precedence rule anywhere.

  Two smaller consequences worth knowing:

  - A run with no flags is a **single-file** run, because `imports` is denied
    with everything else. Multi-file programs need `--allow-imports`; a bundle
    built by `esdev build` does not.
  - **`runtime:process`'s `args` no longer needs `env`.** The arguments are the
    command line that started the program — the same line the permission flags
    are on — not host state a grant withholds. Left gated, the flip would have
    forced `--allow-env`, and with it the whole environment, onto any script
    that reads its own argv.

- **`--allow-all` / `-A` is a real flag.** It used to be rejected with "there is
  no --allow-all"; it now grants every capability, and is the only thing
  `--deny-<name>` may subtract from.

### Fixed

- **The driver test suites ran without permission grants.** D65 made `esrun`
  deny-by-default and these invoke it directly, so they failed on
  `--allow-imports`. The unit runners now grant `imports`; the integration
  runners grant `imports`, `net` and `env`, and nothing else.

- **The root-jail fixture in `permissions.rs` depended on no ancestor having a
  `package.json`.** Adding one at the repo root for tooling moved the detected
  project root and put the tests' "outside the jail" directory inside it. The
  fixture now carries its own `package.json`, which is both what a real project
  looks like and what stops the tests depending on where the repo is checked out.

- **The install one-liner was broken and 404ing.** `install.sh` and
  `install.ps1` resolved the version from GitHub's `/releases/latest`, which
  returns whichever release was published most recently — since `esdev@0.1.0`
  shipped, that is esdev, so the esrun installer was building
  `…/download/esdev@0.1.0/esrun-linux-x86-64.tar.gz` and getting a 404.

  Both scripts now resolve each binary from the newest tag carrying *its* own
  prefix. Asset names never changed; only the version lookup was wrong.

- **`esrun upgrade` could offer a downgrade.** Per-binary tags
  (`esrun@0.24.0`) are not semver after the `v`-stripping self_update applies,
  so every current tag was silently skipped in the release listing and the
  newest *visible* release was the pre-0.24 `v0.23.0` — reported as "New release
  found! v0.22.0 --> v0.23.0 (NOT compatible)". Release resolution now runs
  through a custom `ReleaseSource` that keeps esrun's own tags (both the current
  `esrun@<version>` and the legacy `v<version>`) and reports each one's bare
  version.

## [0.24.0] - 2026-08-13

### Changed

- **`esrun types` and `esrun types --install` are gone**, and the definitions
  they printed are on npm as
  [`@opentf/esrun-types`](https://www.npmjs.com/package/@opentf/esrun-types).

  ```sh
  npm install --save-dev @opentf/esrun-types   # or: esdev --install-types
  ```

  A command whose entire effect is to write into `node_modules` and rewrite a
  `tsconfig.json` is development tooling, and `esrun` is the binary that should
  have none (DECISIONS D59). It loses the subcommand, the `serde_json`
  dependency, and every `.d.ts` that was baked into it with `include_str!`.
  `esrun types` now names its replacement rather than failing as an unreadable
  script path.

  Nothing else about `esrun`'s command line changed. `upgrade` stays: it is
  reachable only by typing it, never from guest JS and never on an ordinary run,
  and it is how an installed `esrun` updates itself.

- **The run itself moved into a new internal crate, `es-runtime-cli-common`**,
  shared with `esdev` — the baked prelude snapshot, the D38 permission grammar,
  the provider wiring, the drive loop, graceful shutdown and the error block.
  Neither binary keeps a second copy of any of it, so they cannot drift on what
  a permission flag means. `esrun` is unchanged — same flags, same messages,
  same behaviour, verified by its existing 290-test suite passing untouched.

  Two hooks were added to the runtime for `esdev` to use, both `None` here: a
  source transform (how `esdev` strips TypeScript) and a capability observer
  (how `esdev --trace-permissions` reports what a run reached for). `esrun`
  carries neither behaviour, and the debugger's code is not compiled into it at
  all — its build script refuses to build while the switch that would is set.

### Added

- **UDP in `runtime:net`** (DECISIONS D58) — `bind()` returns a
  `DatagramSocket`. Messages, not a byte stream: a datagram arrives whole and
  carries its own sender, which is what a stream would erase.

  ```js
  import { bind } from "runtime:net";

  const sock = bind({ hostname: "0.0.0.0", port: 5353 });
  const { port } = await sock.addr;              // port 0 ⇒ ephemeral

  for await (const { data, address, port } of sock) {
    await sock.send(data, { hostname: address, port });
  }

  await sock.joinMulticast("224.0.0.251");       // mDNS, SSDP, discovery
  ```

  `send`/`receive`, `connect()` to fix a peer (sends need no address, and
  datagrams from anyone else are discarded), `joinMulticast`/`leaveMulticast`,
  `addr`, `close()`, and an async iterator that ends at the close. Socket
  options — `reusePort`, `reuseAddress`, `broadcast`, `ttl`, `multicastTtl`,
  `multicastLoopback` — are set at the bind, and an omitted one leaves the OS
  default rather than a value chosen for you. The address family picks the v4 or
  v6 spelling of each, so an IPv6 socket asking for `broadcast` is an error
  rather than a flag that quietly sets nothing.

  This closes the whole category that needs datagrams and has no workaround:
  DNS, StatsD, syslog, NTP, SNMP, mDNS/SSDP discovery, game and telemetry
  protocols, and any local agent listening on a UDP port.

  **Two capabilities, and this is deliberate.** Binding takes a port —
  `NetListen` — while sending reaches a peer — `Net`. A UDP socket is a server
  and a client at once, so gating it on one grant would be a hole in whichever
  was chosen. A program that only receives needs `listen` alone; one that sends
  needs both, because it cannot send without holding a port that answers.
  `--allow-listen` scopes the bind, and `--allow-net` is checked on **every**
  destination, since one socket sends to as many peers as it likes.

  For embedders: `NetProvider` gains six methods, all defaulted to a refusal, so
  an existing implementation compiles unchanged and simply has no UDP.

  Documented in a [UDP guide](https://es-runtime.opentechf.org/docs/guides/udp)
  — including a call-by-call mapping from `node:dgram`, `Bun.udpSocket` and
  `Deno.listenDatagram` — with the design in the sockets internals page and what
  a forged source address means in the security model.

- **Two UDP benchmark rows**, `udp_echo` (10 000 request/response round trips)
  and `udp_send` (50 000 fire-and-forget datagrams), measured across all five
  runtimes. UDP is not a Web API, so each is measured on its own surface, as the
  hashing rows already are — and Deno's needs `--unstable-net`, which the runner
  now passes rather than recording a runtime that has UDP as one that does not.

  Sending is where we land well (4.0 µs per datagram, second of five, ahead of
  Node and Deno); a round trip is where the promise-per-datagram shape is paid
  for (30.9 µs against Node's 28.8 and Bun's 15.7). Both halves ship, and the
  internals page says which part of the design costs the second number.

- **The rest of the UDP surface** (DECISIONS D58, amended) — seven gaps closed
  in a second pass:

  ```js
  sock.unref();                                   // stop holding the process open
  await sock.sendMany(["a", "b"], "10.0.0.1:514");// one crossing, many datagrams
  const batch = await sock.receiveMany();         // …and the same in reverse
  await sock.joinMulticast(group, { source });    // source-specific (RFC 4607)
  await sock.setTtl(64);                          // the options that can change
  bind({ hostname: "::", port: 0, ipv6Only: true });
  ```

  **`ref()`/`unref()`** is the one that mattered most: a pending `receive()` kept
  the process alive with nothing to say otherwise, so a program that binds a
  socket and stops caring never exited. The receive no longer keeps the loop
  alive by itself — a counter does, the same split `worker_recv` makes — so an
  `unref()` takes effect immediately rather than at the next datagram. Unlike
  Node, a bound socket with **nothing in flight** was already not a reason to
  stay alive, which matches `listen()` here.

  **`sendMany`/`receiveMany`** save the host crossing, not the syscalls, and the
  docs say so. `receiveMany` takes one datagram it waits for plus whatever had
  already queued behind it, and never waits to fill a batch.

  **`datagram.truncated`** reports a datagram that did not fit rather than
  handing back a prefix that looks whole. The receive buffer is now a byte past
  what IPv4 can deliver, which is what makes "filled it exactly" mean anything.
  Buffers are also **pooled per socket** now — a 64 KiB allocation per datagram
  was most of the cost of receiving a small one, and dropping it took the
  round-trip benchmark from 34.9 µs to 30.9.

  **Post-bind setters** for the five options that can change, and deliberately
  none for `reusePort`, `reuseAddress` or `ipv6Only`, which must be set before
  the bind — a setter for those would be one that quietly did nothing.
  `setMulticastInterface` closes the multi-homed gap: the outgoing interface for
  a multicast send had no override.

  Not closed: the round-trip cost of a promise per datagram (`receiveMany` does
  not help a strict request/response exchange, where there is only ever one
  datagram to take), source-specific multicast over IPv6, and a receive rate
  limit.

- The end-to-end multicast test now compiles two platform facts into itself
  rather than assuming Linux: `SO_REUSEPORT` where sharing a port needs it (the
  BSDs, macOS included, where `SO_REUSEADDR` covers multicast addresses only),
  and a tolerated "skipped" where loopback multicast delivery is not something a
  CI runner has. The delivery assertion stays strict on Linux.

### Changed

- **`types/` moved to `packages/types/`**, alongside the other npm packages
  (`@opentf/esrun-postgres`, `@opentf/esrun-redis`) rather than sitting on its
  own at the repository root. `@opentf/esrun-types` is unchanged in content and
  in what it publishes — 15 files, verified by packing it — but its
  `repository.directory` now points at the new path, and the two sibling
  packages that depend on it by `file:` path were repointed with it.

### Fixed

- **`YAML.build` and `TOML.build` no longer route through `serde_json::Value`.**
  Both converted the guest value to a `serde_json::Value` and handed that to the
  target format's serializer — which is only correct while `serde_json` has no
  private representation of its own. Under its `arbitrary_precision` feature it
  does: every number serializes as a one-key map named
  `$serde_json::private::Number`, so `YAML.build({ a: 1 })` produced
  `a:\n  $serde_json::private::Number: '1'`.

  Nothing in the runtime asks for that feature, and that is the point — Cargo
  unifies features across everything built together, so **another crate anywhere
  in the workspace enabling it silently changed what the runtime emitted**. It
  was found exactly that way, when `esdev`'s bundler (which depends on it) was
  added and the conformance suite failed only under `cargo test --workspace`.

  Both builders now construct `serde_yaml::Value` and `toml::Value` directly, so
  there is no third format's private representation to leak. This is the same
  lesson the file already recorded twice — `toml_to_value` avoids the `toml`
  crate's `$__toml_private_datetime` sentinel, and `msgpack_build` encodes
  straight from the value tree so a `Uint8Array` is not flattened to `null` —
  now applied to the last two paths that did not follow it.

  `TOML.build({ a: null })` still throws, and now says *why* ("TOML has no
  null") instead of relying on the JSON serializer to refuse it by accident.

## [0.23.0] - 2026-08-11

### Added

- **`runtime:hashing`** (DECISIONS D57) — digests, checksums, MACs and password
  hashing. `crypto.subtle` remains the WebCrypto standard; this is the rest of
  what a server hashes for.

  ```js
  import { hash, Hasher, hashStream, hmac, timingSafeEqual, password } from "runtime:hashing";

  hash("sha256", "hello", "hex");            // encoded in the host, not by a loop at the call site
  hash("xxhash3", buffer);                   // a cache key, at a tenth of the cost

  const h = new Hasher("blake3");            // hash what you cannot hold
  for await (const chunk of file.stream()) h.update(chunk);
  h.digest("hex");

  const stored = await password.hash(input); // "$argon2id$v=19$m=19456,t=2,p=1$…"
  await password.verify(input, stored);
  ```

  Fifteen algorithms: SHA-1/2/3, BLAKE3, MD5, RIPEMD-160, xxHash64, XXH3 and
  CRC-32/32C. WebCrypto's spellings work unchanged (`"SHA-256"` and `"sha256"`
  are the same algorithm), so the two APIs do not disagree about what a hash is
  called. Output is a `Uint8Array` by default, or `hex`/`base64`/`base64url`
  encoded in the host.

  Four gaps this closes, all of them outside WebCrypto's scope by design:
  `subtle.digest` is one-shot, so hashing a 4 GB upload meant holding 4 GB —
  a `Hasher` holds a few hundred bytes of state instead, and `hashStream` is the
  same thing in the line it is usually wanted in
  (`await hashStream("sha256", request.body, "hex")`). Encoding was a
  byte-by-byte loop every codebase wrote once and then copied. There was no
  password hashing beyond PBKDF2 — now Argon2id, bcrypt and scrypt, with the
  parameters and salt travelling inside the stored string, so raising the cost
  never invalidates existing hashes and `needsRehash()` says which to replace.
  And a cache key or ETag no longer pays for collision resistance against an
  adversary who does not exist.

  **No capability, for any of it.** Hashing reads nothing and reaches nothing,
  so every function works under `--deny-all` — `runtime:serialization` is the
  precedent. The one exception is `password.hash()`, which needs a fresh salt:
  rather than let an op help itself to entropy, the module draws it in JS from
  `crypto.getRandomValues`, so hashing a password needs `Entropy` because it
  genuinely needs randomness, and **verifying one needs nothing** — a service
  that only checks passwords is granted nothing at all.

  Two deliberate refusals. `hmac` rejects the checksums, because a MAC built on
  CRC-32 is not a MAC, and rejects BLAKE3, which has its own keyed mode rather
  than this one. And bcrypt refuses a password past 71 bytes rather than
  truncating it — truncation quietly makes two different passwords the same
  password — while *verification* still truncates, since a stored hash may have
  been written by one of the many implementations that do.

  Password hashing runs on the calling thread and blocks it, which is documented
  rather than hidden: offloading it would have required the `TaskSpawn`
  capability and made password hashing need authority that hashing does not.
  MD5 and SHA-1 ship plainly, for the interop they are still needed for.

  Verified by 14 unit tests (a published vector for each of the fifteen
  algorithms, incremental agreeing with one-shot for all of them, the RFC
  2202/4231 HMAC vectors, every password algorithm round-tripped, both bcrypt
  boundaries) and 15 end-to-end CLI tests (the module surface, every encoding
  and input type, agreement with `crypto.subtle` on the four shared digests, and
  the capability claim under `--deny-all`). Documented per D27: DECISIONS D57,
  `docs/API.md`, `types/runtime-hashing.d.ts`, the site's `api/hashing` page,
  the [Hashing guide](https://es-runtime.opentechf.org/docs/guides/hashing), and
  the Node and Bun migration guides.

- **A `hashing` benchmark group** — three rows, each runtime measured on its own
  surface (`runtime:hashing`, `Bun.CryptoHasher`, `node:crypto` `createHash`):
  `hash_hex` (20 000 × SHA-256 of 4 KiB to a hex string, synchronously),
  `hash_chunks` (200 × SHA-256 over 4 MiB in 64 KiB chunks — the incremental
  path `crypto.subtle.digest` cannot express at all), and `hash_fast` (20 000 ×
  a non-cryptographic hash of 64 KiB, which Node, Deno and LLRT have no
  standard-library answer for and record as n/a).

  `hash_hex` and the existing `sha256` row are the **same work** — 20 000 ×
  SHA-256 of the same 4 KiB buffer — one asynchronous through `crypto.subtle`,
  one synchronous. Together they separate access pattern from hash speed: esrun's
  two numbers are 6% apart (327 / 308 ms), Node's are 3.2× apart (504 / 158),
  because `crypto.subtle` here *is* the synchronous op with a resolved promise
  around it. So esrun wins the async row and loses the sync one, and neither
  result is about the hash function.

  What is about the hash function is the remaining ~2×, and **esrun trails**:
  277 MB/s against Node's 580 on the SHA-256 compression function, level with
  LLRT. The README has always caveated `sha256` as "not a claim that RustCrypto
  beats BoringSSL raw"; this measures it. The reference machine has no SHA-NI, so
  both sides run software SHA-256 and OpenSSL's hand-written AVX2 assembly wins —
  `sha2` selects an SHA-NI backend at runtime on hardware that has the
  instructions, which this one does not. `hash_fast` is the row that says what
  the module is for: 16× the data in a fifth of `hash_hex`'s time.

### Fixed

- **Benchmarks tables in dark mode** — `BenchStandings` (Standings at a glance), `WsSweepTable`, `Http2Table` and `RuntimeVersions` now carry `dark:` borders, backgrounds and text (`dark:border-zinc-800`, `dark:bg-zinc-900`, `dark:text-zinc-100/400`, `dark:text-emerald-400`) matching `BenchCard`/`MemorySafetyTable`, so all tables on `/docs/benchmarks` remain readable.

### Changed

- **Published benchmark data re-measured on 0.22.0.** The module carried numbers
  taken on 0.17.0; every charted row is now measured on the shipping build, at
  `WORKLOAD_RUNS=12` (recorded in `method.max_workload_reps`) rather than 5,
  because `fsread_large` needed more samples to corroborate its floor and the
  publish gate refused the run until it did.

  Three cells moved more than 25%, all esrun's large-file fs rows.
  `fswrite_large` (20.8 → 57.8 ms) and `fsappend_large` (5.8 → 18.2) are **not a
  regression**: `9c629ba` fixed `write()` resolving before its bytes landed above
  64 KiB, so the old figures were timing a write that had not finished. Every
  other runtime's fs cells are stable to within ~2% across the two runs, and the
  *small* write/append rows did not move at all — which is what that fix
  predicts, since the sub-64 KiB branch is a synchronous `std::fs` write. The
  third, `fsread_large` (67.8 → 42.4), is an improvement; reads were not touched
  by that fix and the peers moved ~9% too, so part of it is environmental.

- **Standings ordered by total medal count** — “Standings at a glance” now sorts by 🥇+🥈+🥉 descending (tie-break golds then silvers, stable fallback to `ORDER`), so `Bun → esrun → Deno → Node → LLRT`.

- **Site config version** — `website/otfw.config.js` `docs.version` bumped `v0.16.0 → v0.22.0` to match `workspace.package.version`.

## [0.22.0] - 2026-08-10

### Added

- **Temporal by default in `@opentf/esrun-postgres`.** `timestamptz` arrives as a
  `Temporal.Instant`, `timestamp` as a `Temporal.PlainDateTime`, `date` as a
  `Temporal.PlainDate`, `time` as a `Temporal.PlainTime` and `interval` as a
  `Temporal.Duration` — because `Date` cannot say what those columns are. A
  `timestamp without time zone` is not an instant, a `date` is not midnight
  anywhere in particular, and an interval is not a number of milliseconds; every
  one of those had to be guessed at before, and the guess was wrong somewhere.
  `connect(url, { driver, temporal: false })` restores `Date` and strings for
  code that was written against them.

- **`LISTEN`/`NOTIFY` in `@opentf/esrun-postgres`**, under `runtime:db`'s
  subscription surface (see *Changed*). The connection is given over to a read
  loop on the first `subscribe()` and then refuses queries with
  `ERR_DB_CONNECTION_BUSY`, which is how you would deploy it anyway — a
  connection that must notice a notification promptly should not be queued
  behind a report query. Channel names are quoted as identifiers, subscribing
  awaits the server's confirmation, and a handler that throws is reported rather
  than allowed to stop the loop.

- **The `PG*` environment in `@opentf/esrun-postgres`** — `PGHOST`, `PGPORT`,
  `PGUSER`, `PGPASSWORD`, `PGDATABASE`, `PGAPPNAME`, `PGSSLMODE`,
  `PGCONNECT_TIMEOUT`, read the way every libpq tool reads them. Below the URL
  and below explicit options, so they are defaults rather than overrides. A
  program running **without the `Env` capability** is not asking for libpq's
  defaults, so a refusal there is not an error: it means no defaults, and the
  connection string stands on its own.

- **`@opentf/esrun-types` publishes all of itself**, and is built with
  TypeScript 7. `globals.d.ts` and the `runtime:*` module declarations shipped
  incomplete before, so a program could typecheck against a module the package
  did not actually describe.

- **`commandTimeout` in `@opentf/esrun-redis`** (and `?command_timeout=`), which
  **destroys the connection** when it expires. Redis cannot cancel a command in
  flight, so the reply is still coming; a client that gave up but kept the
  connection would read it as the next command's answer and every later value
  would be one behind. It reports `ERR_DB_TIMEOUT` rather than
  `ERR_DB_CONNECTION_LOST` — the lost connection is the consequence, the
  deadline is the cause — and with `reconnect` the cost is one dropped socket.

- **`hscanIterator`, `sscanIterator`, `zscanIterator`** to match `scanIterator`;
  the `LPOP`/`RPOP` count form, which answers an array where the countless form
  answers a value, as Redis does; and `?binary=` on the connection string.

- **More command families in `@opentf/esrun-redis`** — streams (including
  consumer groups), geo, bitmaps, HyperLogLog, hash-field TTLs (Redis 7.4+),
  and `setrange`/`getrange`/`lpos`/`sintercard`/`zmscore`/`zrangestore`.

  Stream entries are `{ id, fields }` rather than the nested arrays the wire
  uses, and `xread` keys its result by stream — RESP3 sends a map there where
  RESP2 sends an array of pairs. `hexpire` and friends report Redis's own
  per-field numbers rather than flattening four outcomes into a boolean. An
  unbounded `XREAD BLOCK 0` is refused like every other blocking command.

- **Sentinel support in `@opentf/esrun-redis`** — the `redisSentinel` driver
  asks the sentinels where the master is and connects there,
  trying each in turn and promoting the one that answered. The address is
  **verified** with `ROLE` before it is used, because a sentinel mid-failover
  hands out a server that has just become a replica and writing to a replica
  loses the writes silently.

  A failover does not close the connection — the old master is demoted, not
  killed — so it is invisible to every other recovery path. A `READONLY` reply
  on a Sentinel-backed connection is therefore treated as *the master moved*:
  the connection is re-resolved and the command retried. `RedisOptions.resolve`
  is the small seam underneath, called before every dial including
  reconnections.

- **Cluster support in `@opentf/esrun-redis`** — the `redisCluster` driver reads
  the topology with `CLUSTER SLOTS`, keeps a pool per primary, hashes keys to
  slots (CRC16/XMODEM with hash-tag rules, checked against published vectors)
  and follows `MOVED` and `ASK` redirects. `ASK` is distinguished properly: it
  is preceded by `ASKING` on the same connection and does **not** update the
  slot map, because treating it as a `MOVED` during a resharding would point
  every later key at a node that does not own it yet.

  Routing is treated as an optimization — a cluster corrects a client that
  guessed wrong, so a wrong guess is slow rather than incorrect — which is why
  the key-extraction table is modest rather than a copy of every command Redis
  ships. The cases handled specially are the ones where argument 1 is not a key:
  `EVAL` and friends route by the key after `numkeys`, not by the script text.

  A transaction spanning two slots is refused **before** it is sent, naming hash
  tags as the fix rather than relaying `CROSSSLOT`. A pipeline may span nodes: it
  is split per node, each group stays one round trip, and the groups run
  concurrently. Everything goes to primaries; replicas are read from the
  topology and deliberately ignored.

- **Reconnection in `@opentf/esrun-redis`** — `{ reconnect: true }`, or an
  object with `attempts`/`delay`/`maxDelay`. **Off by default**: turning it on
  changes what a thrown error means, and a `Pool` does not need it, since
  replacing a dead connection is reconnection with none of the state questions.
  It is lazy — the next command reopens, so an idle connection does not spend
  the process's life dialling a server that is down — except for a subscriber,
  which reopens from its read loop because nobody is going to call it.

  What is restored is what is safe to restore: the handshake, the selected
  database, the client name, and every subscription. What is not, deliberately:
  the command in flight (it was written, and whether the server ran it first is
  unknowable — replaying `INCR` would double-count), a `WATCH` (the server
  forgot it, so the next `EXEC` fails with `ERR_DB_SERIALIZATION_FAILURE` rather
  than succeeding on a guarantee nobody is making), an open `MULTI`, and any
  message published while the connection was down.

  One retry is allowed and it is precise: a command whose **write** failed never
  reached the server. Without it every server restart would cost each live
  connection one spurious error, since nothing notices a closed socket until
  something uses it.

- **Pipelining in `@opentf/esrun-redis`** — `pipeline()` builds a batch with the
  whole command surface on it and sends it in **one round trip**. Measured on
  loopback, where a round trip is nearly free: 500 `INCR`s took 102 ms one at a
  time and 6 ms pipelined.

  `executeMany` now uses it, so a Redis batch costs one round trip rather than
  one per set. Two differences from a SQL backend's batch, both following from
  Redis: it is still **not atomic** (`supports.transactions` is false, so there
  is no transaction to wrap it in), and every set is *attempted* — where the
  default loop stopped at the first failure, a pipeline has already sent them
  all, so a failure is reported after the rest have run.

  A pipeline is explicitly not a transaction: another client's commands may land
  among the batch and one failing does not stop the rest. `multi()` is the one
  that asks the server for isolation, and the two are the same builder differing
  only in whether the batch is wrapped in `MULTI`/`EXEC`.

- **`MULTI`/`EXEC` in `@opentf/esrun-redis`** — `multi()` returns a transaction
  with the whole command surface on it, `exec()` sends it, `watch()`/`unwatch()`
  do optimistic locking. Commands are **buffered** rather than sent as they are
  written, so a transaction is one round trip and a **pool** can run one — there
  is nothing to hold a connection for until `exec()`.

  It is deliberately **not** wired to `runtime:db`'s `transaction(fn)`, and
  `supports.transactions` stays `false`: `MULTI` applies its commands with
  nothing interleaved but does not roll back one that fails at exec time, so a
  `transaction(fn)` on top would commit half a body that threw. `exec()`
  therefore hands per-command errors back **in place** rather than throwing —
  the other commands applied, and throwing would discard their results — and
  answers `null` when a `WATCH`ed key changed, with the queued commands settling
  on `ERR_DB_SERIALIZATION_FAILURE`. The one case that is all-or-nothing is a
  command the server refuses at *queue* time, which makes `EXEC` fail with
  `EXECABORT`; that throws, with the queue-time reason attached.

- **Blocking commands in `@opentf/esrun-redis`** — `blpop`, `brpop`, `blmove`,
  `bzpopmin`, `bzpopmax` and `wait`, each taking its timeout as a **required**
  argument (seconds for the pop family, milliseconds for `wait`), and each
  answering a named shape rather than a positional array: `{ key, value }` for a
  pop, `{ key, member, score }` for a sorted-set pop. Plus `consume(keys)`, an
  async iterator over a list as a queue, which polls with a *bounded* pop even
  though it loops forever — that is what makes an abandoned loop or an aborted
  signal stop it, where an unbounded wait could never notice.

  The indefinite form is now allowed on a connection opened with
  `{ blocking: true }`, which is the caller saying that tying that one up is the
  point. A pool **strips** the option: a pool's premise is that its
  connections come back, and honouring it there would hand out connections that
  can leave circulation permanently.

- **Pub/sub in `@opentf/esrun-redis`** — `subscribe`, `psubscribe`, `ssubscribe`
  and their unsubscribes, with per-channel handlers, an `onMessage` catch-all,
  and `publish`/`spublish`/`PUBSUB` introspection on the ordinary command
  surface. The first `subscribe` **gives its connection over to a read loop**,
  which then refuses ordinary commands with `ERR_DB_CONNECTION_BUSY` — over
  RESP2 that is the protocol's own rule, since a subscribed connection accepts
  nothing but the subscribe family, and over RESP3 it is because the loop owns
  the reader. Open a second connection for it.

  Subscribing is **confirmed** before it resolves, so a publish straight after
  cannot race it and a subscribe the server refuses fails at the call rather
  than silently never firing — the loop owns reading while a `SUBSCRIBE` only
  needs writing, and TCP is full duplex. A handler that throws is reported to
  `onSubscribeError` and the loop continues, since it is the only thing reading
  the socket and one bad handler must not silently stop every other
  subscription. Works over both protocols, which are genuinely different paths:
  RESP3 delivers messages as push frames, RESP2 as ordinary arrays.

  A raw `call(["SUBSCRIBE", …])` is still refused, now pointing at the method
  instead of calling the feature unsupported. `MONITOR` remains refused: one
  reply per command cannot represent a firehose of every command the server runs.


- **`@opentf/esrun-redis`** — a Redis driver, and the second proof that a socket
  backend needs no new Rust (D56 named Redis in the sentence stating that test).
  RESP2 and RESP3 over `runtime:net`, `HELLO` negotiating protocol and
  authentication in one round trip, ACL users, `rediss://` TLS, and a connection
  pool. It ships **two surfaces over one connection**: a `Redis` client with the
  commands spelled as Redis spells them, and a `runtime:db` backend registering
  `redis:` and `rediss:`. Lives in `packages/redis`, versioned separately.

- **The query-AST form actually works.** D56 put `query(q: string | QueryAst)` in
  the contract from the first release so that "an engine which never speaks SQL
  can be a first-class backend rather than a special case" — but `normalizeQuery`
  refused every AST unconditionally, so no backend could take one. A backend now
  declares which forms it takes with **`dialect.supports.sqlText`** (default
  `true`) and **`supports.queryAst`** (default `false`), and the form it does not
  take is refused with `ERR_DB_QUERY_FORM` in either direction. `redis:` is the
  first backend to take an AST: a command is `queryAst(["GET", key])`.

- **`dialect.supports.transactions`** (default `true`). A backend without
  transactions makes `transaction(fn)` refuse with `ERR_DB_UNSUPPORTED` rather
  than emit a `BEGIN` it has never heard of, and its `executeMany` runs without
  one — so a batch is **not** atomic there, which is now declared rather than
  assumed. Redis says `false`: `MULTI`/`EXEC` queues commands but does not roll
  back one that fails at `EXEC` time, so a `transaction()` built on it would
  commit half a body that threw.

- **`_beginTransaction` / `_commitTransaction` / `_rollbackTransaction`** on
  `BaseConnection`, defaulting to exactly the SQL they replaced. They are methods
  so that a backend which does not speak SQL can still have real transactions.

### Changed

- **Binary result formats in `@opentf/esrun-postgres`**, which is a performance
  change big enough to be a behavioural one. Numbers, timestamps, booleans and
  UUIDs are read as bytes rather than parsed from digits: a numeric scan went
  from 72 ms to 51.6 ms — 1.46× `postgres.js`, where it had been 7% ahead — and
  decoding fell from **54% of the scan to 8%**. Both formats decode to identical
  values, which the test suite checks column by column rather than assumes.

- **Each statement is prepared once in `@opentf/esrun-postgres`**, cached per
  connection and bounded by `preparedStatementCacheSize` (default 100, `0`
  disables). Worth 6% on its own; the reason it is not worth more is the reason
  binary formats were: a round trip is a fixed cost per query, so it is the whole
  cost of a point query and negligible against ten thousand rows, where decoding
  is what scales. A cached plan the server has invalidated is detected and
  re-prepared rather than failing the query.

- **`runtime:db` takes a driver rather than a registry.** `connect(url, options)`
  now requires `options.driver` — a value you import and pass, not a global
  installed by importing a package for its side effects:

  ```js
  import { connect, sqlite } from "runtime:db";
  import { driver as postgres } from "@opentf/esrun-postgres";
  import { driver as redis } from "@opentf/esrun-redis";

  const db = await connect("sqlite:./app.db", { driver: sqlite });
  const pg = await connect("postgres://user@host/app", { driver: postgres });
  const r  = await connect("redis://localhost", { driver: redis });
  ```

  `registerBackend` and `backendSchemes` are **gone**, and with them the
  reserved-scheme list and the rule that a built-in could not be replaced. A
  scheme was a global name a package claimed by being imported: which backends
  existed depended on which modules had been evaluated, the dependency was
  invisible at the call site, and two implementations of `postgres:` could not
  coexist. A driver is an ordinary value, so none of that arises — and because
  the driver is part of the call, `connect` returns **that driver's**
  connection, which is what removed the second entry point every driver had
  grown.

  The built-in SQLite backend is now `sqlite`, an ordinary driver defined with
  the same `defineDriver` a third party uses; `connect` knows nothing about it
  that it does not know about a driver published this morning.

- **Pooling is an option on `connect`, and lives in one place.**
  `connect(url, { driver, pool: true })` — or `pool: { max: 20 }` — returns a
  `PooledConnection` presenting exactly the surface one connection does, plus
  `size`, `idle`, `pending` and `withConnection(fn)`. The borrow-per-call
  discipline, returning a connection when a streaming result ends, and
  destroying one that came back dirty were written out once per driver before
  this; they are now written once, in `runtime:db`.

  A connection answers `usable` (worth using at all) and `reusable` (fit for the
  next caller) — the one question a protocol-blind pool cannot decide for
  itself, now asked by one name on every backend instead of three
  (`status === "I"`, `clean`, nothing at all).

- **`@opentf/esrun-redis` is one object per connection.** `Redis`,
  `createClient`, `createSubscriber`, `createPool`, `createCluster`,
  `createSentinelClient` and `createSentinelPool` are gone. The command surface
  is now on `RedisConnection` itself, so the connection `connect` returns
  answers both vocabularies — `r.set("k", "v")` and
  `r.query(queryAst(["LRANGE", …]))` on the same object — and the package
  exports three drivers instead: `driver`, `redisCluster`, `redisSentinel`.
  Which client you get follows from the driver you passed rather than from which
  of seven functions you called.

- **One subscription surface, on every connection.** `LISTEN`/`NOTIFY`, Redis
  pub/sub and a change stream are one concept, and the two shipped drivers had
  each invented a name for it: PostgreSQL had `listen`, `unlisten`,
  `onNotification`, `listening` and `onListenError`; Redis had `subscribe`,
  `unsubscribe`, `onMessage`, `subscribed` and `onSubscribeError`. The portable
  spelling is now the second one, on every `Connection`:

  ```js
  await conn.subscribe("orders", (payload, { channel }) => …);
  conn.onMessage = (payload, context) => …;
  await conn.unsubscribe("orders");
  ```

  `supports.subscriptions` declares it; a backend without it refuses by name.
  So does a **pooled** connection — a subscription needs a connection that does
  not come back, which is the opposite of a pool's premise — and so does a
  cluster client, where Redis pub/sub is not cluster-aware and a cluster-wide
  subscribe would deliver some messages and silently miss others. A driver
  implements `_subscribe`/`_unsubscribe` and inherits the rest.

- **`supports.sqlText` is `supports.queryText`.** The flag means "takes query
  text", and a backend speaking Cypher, N1QL or a language of its own takes text
  without being a SQL backend — which the old name made unsayable.

- **`executeMany` reports per parameter set.** `ExecuteResult.results` carries
  one result per set wherever the backend can report them, which the default
  batch path always can, since it ran them one at a time and had them in hand.
  Without it a batch of inserts against a backend that generates keys was a
  batch whose keys were unreachable: the aggregate carries only the last.

- **`Row<V>` and `Rows<R>` are generic.** `DbOutput` describes what the built-in
  backend produces, and the types said it described every backend while
  `@opentf/esrun-postgres` was already returning `Temporal` values and parsed
  JSON. A driver now declares what it produces (`Rows<PgRow>`); the portable
  `Connection` types its rows `unknown`, because an unknown backend decodes what
  it likes, and `sqlite` narrows back to `DbOutput`.

- **`ERR_DB_THROTTLED` and `ERR_DB_NOT_FOUND`** join the portable codes.
  Throttling is the service shedding load — a quota, a rate limit, a connection
  cap — as distinct from `ERR_DB_BUSY`, which is one resource held by someone
  else; both shipped backends map real conditions onto it (PostgreSQL's
  `53300`/`53400`, Redis's `MAXCLIENTS`). `NOT_FOUND` is for a backend asked for
  one named thing that has none, which is not the same as a query that matched
  nothing.

- **Every driver package exports its driver as `driver`, and nothing as a
  default.** One import shape for every backend, and `{ driver }` is the whole
  of the option:

  ```js
  import { connect } from "runtime:db";
  import { driver } from "@opentf/esrun-postgres";

  const db = await connect("postgres://user@host/app", { driver });
  ```

  Two drivers in one module are told apart with `as`
  (`import { driver as postgres }`). A default export alongside named ones was
  two ways to import the same value, which is the confusion that makes people
  check the README to write an import.

- **Rows may cross as `records`, not only as bytes.** `Rows.fromObjects(records)`
  — or a `RowSource` answering `{ records, done }` with `defineRecordShape` —
  is for a backend whose values are already JavaScript: a document store
  answering JSON, a graph or vector service over HTTP, an engine holding
  objects. The byte layout stays exactly what it was for the backends it was
  designed for, which is every wire protocol.

  It came out of finding that `@opentf/esrun-redis` was encoding a reply it
  already held in memory into the layout so that `decodeBatch` could take it
  apart again — duplicating the kit's value tags to do it. That path is gone:
  the driver loses about ninety lines and its copy of the tags, and nothing
  downstream can tell which kind of batch it is reading.

- **`withConnection`, `usable` and `reusable` are on every connection.**
  `withConnection(fn)` was on a pool only and `usable`/`reusable` on a single
  connection only, so code holding "a connection" had to know which kind it
  held — an ORM would have had to demand a pool or duplicate itself. A cluster
  client refuses `withConnection` by name, since its keys may live on different
  nodes.

- **`dialect.supports` takes a driver's own capability flags**, so a backend can
  tell an ORM about a vector index or a full-text mode the ORM has never heard
  of; and `ExecuteResult.lastInsertRowid` is typed for the string keys every
  backend outside SQLite's family generates.

- **`@opentf/esrun-postgres` exports its driver.** `connect` and `createPool` are
  gone from the package; import its `driver` and pass it. `PgPool` is now `PgPooled`, a `PooledConnection` subclass that adds
  `executeScript` and nothing else.


- **`BaseConnection._executeMany(query, sets)`** takes the whole
  `NormalizedQuery` rather than just its `text`, which is `null` for a backend
  that took an AST. A driver-tier signature change; only the built-in `sqlite:`
  override needed following, and `@opentf/esrun-postgres` does not override it.

- **`NormalizedQuery`** gains `ast`, and its `text` is now `string | null`.
  Exactly one of the two is non-null.

- **The conformance suite is form-aware.** A check written in SQL is **skipped
  with a reason** against a backend declaring `supports.sqlText: false`, rather
  than failed — a check a backend cannot express is not a finding. `skipped`
  results carry `reason`, because a count with no explanation is how a driver
  author concludes they passed something they never ran. Two checks were
  generalized to run everywhere; the query-form check previously hardcoded the
  AST as the wrong form, which is backwards for a backend that takes one.
  `sqlite:` and `postgres:` still run and pass all fifteen.

### Fixed

- **`@opentf/esrun-redis` refuses a blocking command with no timeout.**
  `BLPOP`, `BRPOP`, `BLMOVE`, `BRPOPLPUSH`, `BZPOPMIN`, `BZPOPMAX`, `BLMPOP`,
  `BZMPOP`, `WAIT`, `WAITAOF` and `XREAD`/`XREADGROUP` `BLOCK` hold the
  connection for as long as they block — inherent, since the server sends no
  reply until it has one. A **bounded** wait is a stall the caller chose and is
  allowed; a timeout of `0` means forever, which is a connection that never
  comes back, and through a pool one that is out of circulation for the life of
  the process while every other caller fails on `acquireTimeout` pointing at
  pool exhaustion rather than at the cause. The unbounded form now throws
  `ERR_DB_UNSUPPORTED` before anything reaches the wire. Redis keeps the timeout
  in three different places — last for `BLPOP`, first for `BLMPOP`, behind the
  `BLOCK` keyword for `XREAD` — and the check knows all three, including that a
  stream legitimately named `BLOCK` is not the option.

## [0.21.0] - 2026-08-09

### Added

- **`@opentf/esrun-postgres`** — the first ecosystem package, and the proof of
  D56's central claim: a PostgreSQL driver that is **entirely JavaScript over
  `runtime:net`**, adding no native code to the runtime. SCRAM-SHA-256 over
  WebCrypto, the extended query protocol, `SSLRequest`/`startTls` negotiation,
  and SQLSTATE mapped onto the portable error codes. It passes the same
  `runBackendConformance()` suite the built-in `sqlite:` backend does, against a
  real PostgreSQL 18. Lives in `packages/postgres`, versioned separately.

- **`runtime:net` `connect({ ca })`** — extra trust anchors as PEM, for a server
  presenting a certificate from a private authority. **Added** to the built-in
  roots rather than replacing them, so naming one does not quietly stop the
  program trusting every public one, and it can only make verification accept
  more certificates: the hostname and chain checks still run. Carried through
  `startTls()` as well, which is what a `postgres://` connection needs. There is
  no option to skip verification.

- **`{ signal }` on `query` and `execute`** — portable cancellation. Aborting
  asks the backend to cancel and waits for it, so the connection stays usable,
  and the rejection carries the signal's own `reason` rather than the backend's
  word for a cancelled statement. `sqlite:` interrupts a running statement
  through the new `EmbeddedDb::cancel` seam; `@opentf/esrun-postgres` cancels
  over the protocol. A driver that implements neither still rejects its caller.

- **`Pool`** in `runtime:db`'s driver tier — a protocol-blind resource pool with
  a bounded size, a waiter queue, lazy idle sweeping (not a timer, which would
  keep the loop alive and stop a finished program exiting), and the
  `release(clean)` contract D56 specified: the driver asserts whether a
  connection is fit to reuse, and anything not explicitly clean is destroyed.
  `@opentf/esrun-postgres` is its first consumer, which is what D56 said would
  justify shipping it.

- **`ERR_DB_CONNECTION_BUSY`** joins the portable error codes: the connection is
  already streaming a result set. Distinct from `ERR_DB_BUSY`, which is the
  database refusing — this one is the client's own connection, and only the
  caller draining that result can free it. Every wire protocol has the
  constraint, which is why it is portable rather than one backend's problem.

- **`runtime:db`** — databases, in two tiers (D56). The **application tier** is
  `connect()`, a `sql` tagged template that binds every interpolation as a
  parameter, and the `Connection` / `Rows` / transaction surface they return.
  The **driver tier** is what a third party needs to add a backend: a scheme
  registry, the row-shape and batch decoder, the parameter encoder, `Dialect`,
  the portable `DbErrorCode` table with `mapError`, and `BaseConnection` — from
  which transactions, savepoints, and the abandoned-cursor discipline come for
  free and, more to the point, come out the same across backends.

  The first backend is `sqlite:`, which names a file format and a SQL dialect
  the way `postgres://` names a wire protocol — implemented by `turso_core`,
  which appears nowhere in the API and can be replaced without one. Result sets
  stream a batch at a time, so a table larger than memory costs one batch;
  stopping early closes the cursor. A 64-bit integer round-trips as a `bigint`
  rather than rounding through a double. Encryption takes its key from the
  options object, and a key passed in the connection string is refused rather
  than quietly honoured. Networked backends are next and need no new Rust:
  Postgres will be JS over `runtime:net`.

  `sqlite::memory:` opens a database that exists only in memory and **needs no
  capability** — it names no file and touches no filesystem, so it is the one
  open that works under `--deny-all`. Each connection gets its own; the named
  form (`:memory:name`), which in SQLite means *sharing* one, is refused rather
  than quietly not sharing.

  `runBackendConformance()` is exported so a third-party driver can demonstrate
  it behaves like the built-ins rather than intend to — thirteen checks covering
  column order, parameter binding, null handling, streaming and early exit,
  transactions and savepoints, the portable error codes, and the refusals. The
  built-in backend runs it too, against both a file and an in-memory database.

  `executeMany(sql, rows)` runs one statement over many parameter sets in a
  single boundary crossing, preparing it once — 50k inserts go from 1832 ms to
  312 ms. It runs as one transaction unless one is already open. And a result
  small enough to fit one batch now comes back **with the query itself**: no
  cursor is opened, so a lookup by primary key costs one crossing instead of
  three (`rows.exhausted` reports which happened). `executeMany` works on every
  backend from the day the backend exists: `BaseConnection` supplies a default
  that loops `_execute` inside the same transaction, so a driver overriding it
  is buying speed rather than the feature.

- **`EmbeddedDb` provider trait** — the seam an in-process database engine
  arrives through, and the only database seam below the op boundary (D56).
  Networked backends are built in JS on `NetProvider`, so adding Postgres or
  MySQL adds nothing here. Rows cross as one flat byte run in Postgres
  `DataRow` layout rather than as a value tree, so a single decoder serves the
  embedded engine and the wire protocols alike; batches are bounded by bytes
  rather than row count. Embedder-facing only for now — `runtime:db` is not yet
  exposed.
- **`SystemEmbeddedDb`** — the `EmbeddedDb` implementation behind `sqlite:`,
  over `turso_core`. The engine runs against a **jailed VFS**: every file it
  opens, including the write-ahead log and shared-memory index it opens without
  being asked, resolves through the same root jail and `--allow-read` /
  `--allow-write` scopes that back `runtime:fs`. Queries stream through a
  cursor, and each fetch runs off the event loop.
- **Embedded-database ops** (`db_open`, `db_open_read_only`, `db_query`,
  `db_fetch`, `db_execute`, `db_close_cursor`, `db_close`) and
  `HostProviders::with_embedded_db`. Opening needs `FileRead`, and `FileWrite`
  as well unless it is read-only — so a database is scoped by `--allow-read` /
  `--allow-write` exactly as a file is, and `runtime:db` adds no capability of
  its own. Ids are owned per agent (D50). Parameters cross as one tagged buffer
  rather than a value array, which is what lets a bigint bind as a 64-bit
  integer instead of rounding through a double.

### Fixed

- **`@opentf/esrun-types` published an incomplete package.** `globals.d.ts` and
  `runtime-websocket.d.ts` were referenced by `index.d.ts` and absent from the
  `files` list, so an installed copy could not resolve its own references. The
  list is a glob now — it cannot fall behind a new module — and a test asserts
  every declaration file is publishable.

- **`types/runtime-db.d.ts`**: `DbError` and `Rows` are declared constructible,
  and `ColumnDecoder` returns `unknown` rather than the built-in backend's value
  set. All three were found by writing a driver against the declarations rather
  than by reading them — a backend that decodes `timestamptz` to a `Date` is
  doing its job, and a driver has to be able to build the errors and results it
  returns.

## [0.20.0] - 2026-08-08

### Security

- **A host handle is usable only by the agent that created it.** Sockets,
  listeners, file descriptors, child processes, HTTP servers, in-flight
  requests, WebSocket connections and workers are reached by an integer id. The
  op that *acquires* one is capability-checked; the ops that use it were not, on
  the reasoning that the id could only have come from a checked call. That stops
  being true with more than one agent in the process: providers are shared, their
  ids start at 1 and count up, and `globalThis.__ops` is reachable by design. So
  `new Worker(url, { permissions: [] })` — an agent granted **nothing** — could
  call `__ops.system_kill(1, "SIGKILL")` and kill its parent's child, or
  `__ops.http_respond(2, …)` and answer a request its parent was serving,
  sending its own body to the client. Every op that takes a handle now checks it
  belongs to the calling agent, and refuses with `NotAllowedError` /
  `ERR_FOREIGN_HANDLE` if not. `ws_broadcast` checks every id in the fan-out,
  not just the first (D50).

- **Documented what an `--allow-net` name entry bounds.** The check runs before
  resolution, which the docs said; what they did not say is the consequence. A
  name entry permits a connection to wherever that name resolves, chosen by
  whoever controls the zone and chosen again on every reconnect — including a
  loopback or cloud-metadata address. Write an IP entry where the machine is
  what matters; an IP entry is never satisfied by a name. Behaviour is
  unchanged: checking after resolution instead would have to refuse the private
  addresses that `--allow-net=db.internal:5432` exists to reach.

- **An import-policy path entry now governs a `node_modules` tree.** A module
  inside `node_modules` was judged as the package it belongs to *instead of* as
  a path, so every path entry pointing into one was silently inert:
  `{"deny": ["./node_modules/aws-sdk"]}` denied nothing, parsed without
  complaint, and read as a restriction. A module is now named by a rule if
  either list names it.

- **`--allow-run` matches a program, not a name.** The check accepted a spawn
  whose *basename* was on the list, so `--allow-run=git` admitted `/tmp/x/git` —
  any file called `git` the guest could reach, including one it had just written
  with `--allow-write`. Each entry is now resolved once to a real path (a bare
  name through `PATH`, a path as written) and a spawn is admitted only if it
  lands on the same file. `--allow-run=git` still admits `/usr/bin/git`; it no
  longer admits a different program wearing the name.

- **A `Host` header that is not an authority is refused with `400`.** The
  header was spliced into the absolute URL the handler routes on without being
  checked against the grammar, so `Host: h/admin?` turned `GET /public` into
  `http://h/admin?/public` — a request whose `new URL(request.url).pathname` is
  `/admin`. A client could pick the path the application matched on. The
  authority (the `Host` header, or `:authority` on HTTP/2) must now be a host
  and an optional port; userinfo is refused too, since `Host:
  evil.com@real.host` makes a URL whose visible prefix is not its hostname.

- **`MessagePort` ids are unguessable.** A port id is meant to be transferred,
  so it cannot be owned by one agent the way the handles above are — holding the
  id *is* the authority. They were sequential, so an agent that was never given a
  port could read and write another agent's channel by trying 1, 2, 3. They now
  come from the CSPRNG (D50).

### Added

- **`upgradeWebSocket(request)` — one port for `https:` and `wss:`.**
  `runtime:websocket` `serve()` binds a listener of its own, so a service that
  already had an HTTP server needed a second port and a second certificate for
  its sockets, and there was no TLS WebSocket server at all. Node, Deno and Bun
  all upgrade on the HTTP server instead; this does too.

  ```js
  serve({ port: 443, secureTransport: "on", cert, key }, (request) => {
    if (request.headers.get("upgrade") === "websocket") {
      const { response, socket } = upgradeWebSocket(request);
      socket.onmessage = (e) => socket.send(e.data);
      return response;
    }
    return new Response("api");
  });
  ```

  What comes back is an ordinary connection — `broadcast()` reaches it alongside
  sockets from `serve()`, and `maxBufferedAmount` applies. The handshake headers
  are the host's, since `Sec-WebSocket-Accept` is a digest of a key the handler
  never sees. Returning anything but the `response` declines the upgrade.

  Subprotocols are negotiated with `upgradeWebSocket(request, { protocol })`,
  checked against what the client offered — naming another is a `TypeError`
  rather than a handshake the client silently rejects. `connection.protocol`
  reports the result, and now does so on `serve()`-accepted connections too:
  the provider had always reported it and the prelude was dropping it.

  **Over TLS the client must negotiate `http/1.1`** — browsers do for `wss:`.
  WebSocket over HTTP/2 needs RFC 8441 extended CONNECT, which is still not
  implemented. (D55.)

- **WebSocket send backpressure: `connection.bufferedAmount` and
  `maxBufferedAmount`.** `send()` is fire-and-forget — the WebSocket API has no
  way to report a full buffer — so writing faster than a peer reads never
  stalled the guest; the messages queued on the host, one pending send each,
  bounded by nothing. A peer that stopped reading a fan-out was a memory leak
  with a network interface, which is the gap D47 recorded. Server-side
  connections now expose `bufferedAmount` (the client `WebSocket` always did),
  so a sender can pace itself; and a connection whose queue passes
  `maxBufferedAmount` is closed with `1013` (Try Again Later) rather than held.
  **On by default at 8 MiB**, unlike the connection caps, because the number
  does not depend on what the deployment knows; `0` removes it. It applies per
  connection, including the ones `broadcast()` fans out to, and to client
  connections — a slow server is the same problem from the other end.

- **`maxConnectionsPerIp` on `runtime:http` and `runtime:websocket` `serve()`.**
  `maxConnections` bounds what a deployment spends and nothing else: one peer
  opening every slot filled the server exactly as a thousand peers opening one
  each did, and it was then full for everybody. This is the half that says
  *whose* connections they are — the gap D45 and D47 both recorded, now that
  D44's peer address exists to build it on. A connection over it is **refused**
  rather than held, unlike the whole-server cap: an excess there is legitimate
  traffic queueing for a slot, and an excess here is one client past its share,
  already accepted and holding a descriptor it decides when to release.
  Unlimited by default — **behind a proxy or a NAT it should stay off**, since
  every connection then carries the same source address and a cap would apply to
  the whole service.

- **A bound on a slow request body** — `timeouts.bodyRead` (30s) and
  `timeouts.bodyMinRate` (1024 B/s). `headerRead` stops when the head is
  complete, so a peer that sent a well-formed head and then dribbled its body a
  byte at a time was past every timer the server had: slowloris, one phase
  later. A flat cap cannot answer it — over elapsed time a 100 MiB upload on a
  slow link looks identical — so the deadline is **earned**:
  `bodyRead + received / bodyMinRate`. At the defaults a 100 MiB upload has over
  a day to arrive, a 1 GiB one over a week, and a byte-a-minute peer is closed
  at ~30s. `bodyMinRate: 0` makes `bodyRead` a flat cap; `bodyRead: null`
  removes the bound. A body that runs out reaches the handler as its stream
  erroring with `ERR_TIMED_OUT`.

- `ERR_FOREIGN_HANDLE` — a socket, child process, server, file descriptor or
  request belonging to another agent.

### Fixed

- **A handle you gave up is finished, not foreign.** Ownership checks (below)
  made four ordinary sequences raise `ERR_FOREIGN_HANDLE` — the right answer to
  naming *another* agent's handle, and the wrong one to naming your own after it
  ended. Found by sweeping every op that takes a handle against every prelude
  call path, rather than one at a time:

  - `child.kill()` after the child exited **and** both its pipes were drained
    threw, though `kill()` documents that signalling an exited child is a no-op.
    Closing its `stdin` did too.
  - Reading `request.body` after returning a buffered `Response` threw instead of
    ending. The host drops an undrained body when the response goes out, so from
    the guest's side it is simply over. (A *streamed* response still holds the
    body open — `new Response(request.body)` is the echo case.)
  - `socket.close()` on a socket already consumed by `startTls()` threw; the
    upgrade retires the old id, so the original names nothing.
  - `runtime:wasi` reported such a descriptor as `EIO` rather than `EBADF`.

### Changed

- **An `--allow-read` / `--allow-write` path outside the root jail now adds that
  subtree.** Serving HTTPS was impossible: the cert and key travel inline, so a
  guest reads them itself with `runtime:fs` — which is jailed to the project
  root, while certificates live in `/etc/letsencrypt`, where renewal writes them
  and no project root reaches. The HTTPS examples in our own docs could not run.

  Inside the jail a path list still narrows. Outside it, a path **adds** — and
  only a path typed on the command line can, which is the deployment operator
  naming a location, never guest code. The jail stays the boundary a program
  cannot move: a path neither inside it nor named is still `ERR_JAIL_ESCAPE`,
  `--allow-read` does not make its subtree writable, and module resolution is
  untouched (the loader keeps its own root, so a granted path makes bytes
  readable, not code importable). (D54.)

## [0.19.0] - 2026-08-07

### Added

- **`reusePort` on `runtime:http` `serve()` and `runtime:net` `listen()`.**
  Binds with `SO_REUSEPORT`, so several *processes* can listen on one address
  and the kernel balances new connections across them — how a server runs across
  cores without a front proxy, and how one is replaced without dropping
  connections. Every sharer must set it; a plain bind on a held port is still
  `ERR_ADDRESS_IN_USE`, which is what keeps the flag meaningful. **Unix only** —
  Windows has no equivalent (its `SO_REUSEADDR` lets an *unrelated* process take
  a bound port, a hijacking primitive rather than a load-balancing one), so
  asking for it there is an error rather than a silent exclusive bind that fails
  the moment you scale.

- `ERR_INVALID_PATH` — a path argument that names no valid target: it is empty,
  or it is the filesystem root jail itself and the operation would mutate it.

- `ERR_SAME_FILE` — source and destination name the same file, for an operation
  that would have to read one while truncating the other.

### Fixed

- **`broadcast` refuses an element that is not a connection.** It skipped
  anything it did not recognize, so `broadcast([...room, undefined], msg)`
  delivered to the rest and said nothing, and a list that had somehow filled with
  the wrong type broadcast to nobody and still returned normally — a chat room
  going quiet with every call looking like it worked. A connection's id is set
  when it is constructed and never removed, so its absence is a brand check
  rather than a liveness one: a **closed** connection still carries one and is
  still handed to the host, which owns the live socket table and is the only
  place that question can be answered without a race. Only something that was
  never a connection throws, and the whole iterable is checked before anything is
  sent, so a bad element fails the call rather than half-delivering it.

- **A body stream chunk that is not bytes is a `TypeError`.** Draining a body
  sized each chunk by `.length` and never checked its type. A string chunk was
  accepted and `Uint8Array.set` coerced every character to `NaN`, so
  `new Response(stream).text()` returned that many **NUL bytes** instead of
  failing; a number or object surfaced an internal `RangeError` about offsets.
  The same `.length` is an element count, not a byte count, so a `Uint32Array`
  chunk contributed a quarter of its bytes and a bare `ArrayBuffer` — which has
  no `.length` — arrived **empty**. Chunks are now measured in bytes and
  validated, matching what the `fetch` upload pump and the `serve` download pump
  already did; the three had drifted because each wrote the conversion out
  itself, and there is now one shared implementation.

- **A failing response body says why.** `serve` aborts the connection when a
  body stream yields a bad chunk, which is right — the status line is already on
  the wire, and closing cleanly would claim a truncated body was complete — but
  `serve` has no error hook, so the abort reached the client and left the
  server's own author a connection reset with nothing to go on. The cause is now
  reported where uncaught errors are.

- **A trailing `/` no longer opens a file.** POSIX reads `file.txt/` as "this
  name must be a directory" and the kernel refuses it with `ENOTDIR`, which is
  what Node and Bun surface. Path resolution here canonicalizes, and that drops
  the trailing separator, so `file.txt/` reached the operation indistinguishable
  from `file.txt` and was read, written, stat'd, renamed, copied and removed as
  though the separator had never been written — only `readDir` refused it, being
  the one call whose syscall re-derives the requirement. The requirement is now
  enforced at resolution and reports `ERR_NOT_DIRECTORY`, on the `runtime:wasi`
  door onto the same jail as well. A trailing separator on an actual directory is
  unaffected, and a path that does not exist still reports that, so
  `mkdir("newdir/")` is unchanged.

- **`env` values are coerced to strings.** An environment is a string-to-string
  map, and a string is the only thing a child process can receive — but an
  assignment stored whatever it was given, so `env.PORT = 8080` left a *number*
  in it. `typeof env.PORT` was `"number"`, and handing the object on as
  `new Command(cmd, { env })` then threw "must be a string" for a value the
  program had every reason to think it had set correctly. Assignment now coerces
  (`"8080"`), including through `Object.defineProperty`, which bypassed the write
  path entirely. A symbol has no string value and throws, exactly as in Node and
  Deno. Secret-keyed values wrap the coerced string, so masking is unaffected.

- **Protobuf `encode` accepts the field names the `.proto` declares.** It read
  only the lowerCamelCase JSON name, so a message written with the field names as
  they appear in the schema — `{ user_name: "ada" }` for `string user_name = 1` —
  matched nothing and encoded to a **0-byte buffer**, losing the whole message
  without an error. The proto3-JSON mapping requires both spellings to be
  accepted, and this package's own `fromJson` and `decodeStream` already did;
  `encode` was the outlier. Supplying both spellings of one field is rejected, as
  the mapping also requires. A key matching **no** field now throws rather than
  being dropped, so a typo can no longer encode to a short buffer in silence —
  pass `{ ignoreUnknownFields: true }` to `encode`/`encodeDelimited` for the old
  lenient behaviour. Decoded messages still re-encode unchanged, preserved
  unknown wire fields included.

- **`copy(p, p)` no longer empties the file.** `fs::copy` opens the destination
  truncating *before* it reads the source, so copying a file onto itself wiped it
  and reported success with `0` bytes copied — the backup destroying the original,
  with nothing thrown to notice it by. Copying between two hardlinks to one inode
  did the same, and path equality alone would not have caught it. `copy` now
  refuses when source and destination are the same file, by device/inode on Unix
  and by canonical path elsewhere, with `ERR_SAME_FILE`. Deno refuses the same
  call; Node treats it as a no-op, which is safe but reports success for what is
  almost certainly a caller bug. Copies between distinct files are unchanged and
  still overwrite the destination.

- **A circular or deeply nested argument no longer kills the process.** Every op
  argument is marshaled across the V8 boundary by a recursive descent that ran on
  the native stack with no bound, so `o.self = o` — or a literal nested a few
  thousand deep — exhausted that stack and V8 aborted the whole isolate (`Check
  failed: isolate_->IsOnCentralStack()`). Nothing was catchable and the process
  died, which made `XML.build`, `YAML.build` and `TOML.build` a one-line kill
  switch for any program that built one from untrusted input. The descent now
  refuses a cycle with a `TypeError` and nesting past 256 levels with a
  `RangeError`, both ordinary catchable exceptions. Only the path from the root is
  tracked, so a value reachable twice by different routes is still marshaled twice
  rather than being mistaken for a cycle.

- **Secret masking applies to values the program assigns.** `env.MY_API_KEY =
  "…"` — how a program threads a value it just fetched down to a child — stored
  the string raw, so a key that arrives masked from the host environment stayed
  an unmasked string when the program set it, and leaked in a log line, a
  template or a `JSON.stringify` like any other value. The same key convention
  now applies on write; a value that is already a `Secret` is not wrapped twice,
  and `unmask` still reveals it.

- **`runtime:serialization` has a default export.** It was the only one of the
  nine `runtime:` modules without one, so `import serialization from
  "runtime:serialization"` was a `SyntaxError` for no reason a caller could see.

- **Failures are reported when they happen, not at exit.** An unhandled
  rejection or a throw out of a timer was collected and printed only once the
  drive loop returned — and a listening server keeps that loop alive, so a
  long-running program's failures were invisible for its whole life. They now
  print at the point they occur, while the program carries on; the exit status
  is unchanged, and the final line is only a count, since repeating the messages
  would report each failure twice. The entry module's *own* top-level throw is
  still reported once, by name, as an uncaught exception.

- **`XML.parse` requires a root element.** `""` parsed to `{}` and
  `"not xml at all"` came back as that same string, so anything at all parsed
  "successfully" and `XML.validate` agreed — a caller checking input before
  trusting it learned nothing. Both now reject a document with no element.

- **`path.normalize` keeps a trailing separator.** `normalize("a/b/")` returned
  `"a/b"`, dropping the one thing that says the path names a directory; it is
  kept now, as it is everywhere else, and `join` keeps it too. `resolve` still
  drops it (unless the result is the root) — it answers *which location*, and a
  location is the same one however it is spelled.

- **`runtime:system` no longer exposes its internals.** `Command` and
  `ChildProcess` carried `_collect`, `_output`, `_readable`, `_writable`,
  `_streamDone` and `_maybeRelease` as public prototype members. They are
  private fields and methods now; the prototypes are exactly the documented
  surface.

- **A top-level throw is fatal even with a server running.** The entry module's
  failure was only checked *after* the drive loop returned, and a listener keeps
  that loop alive forever — so a program that threw at top level after starting
  a server had its exception discarded entirely and ran on, serving, with
  nothing reported and no exit. It now stops the drive as soon as the module's
  evaluation fails, exactly as it does without a server.

- **A failed bind no longer looks like a clean shutdown.** `serve()` rejected
  `addr` but *resolved* `finished`, so a server that never bound was
  indistinguishable from one that ran and stopped: `await server.finished`
  returned normally. Both promises now reject with the same error, and only
  `addr` is left for the unhandled-rejection path so one failure is reported
  once. The error is also classified — `ERR_ADDRESS_IN_USE` and friends, with a
  `listen host:port: …` message — where it used to be an uncoded
  `provider error: …` string with nothing stable to branch on.

- **`XML.parse` rejects a truncated document.** Reaching the end of the input
  with elements still open ended the parse quietly, so `"<r>"` produced
  `{"r":{}}` and `"<r><a>1"` silently dropped the `1` — a partial object
  returned as though it were the document. It is a `SyntaxError` now, and
  `XML.validate` agrees: it answered `true` for the same input, because it
  only surfaced errors the reader raised and a truncated document raises none.
  A *mismatched* end tag was already caught, so only this case got through.

- **A name that cannot resolve reports `ERR_DNS`.** It reported the catch-all
  `ERR_IO`: the classifier returns early on any I/O error carrying a specific
  kind, and a resolver failure carries a kind with no stable name — which maps
  to `Io` — so the check that recognises a lookup failure never ran. The
  generic code is now a fallback rather than an early return, leaving the more
  specific classifications (`ERR_CONNECTION_REFUSED`, `ERR_TLS`) untouched.

- **`FormData` stores a `Blob` value as a `File`.** Creating an entry converts
  a plain `Blob` to a `File` named `"blob"` (the standard's "create an entry"
  step), which it was only doing when an explicit filename was passed. Without
  it `fd.get(k) instanceof File` was false and `.name` undefined, so the usual
  way to pick the file parts out of a form skipped them — while the multipart
  body had already written `filename="blob"`, so the wire and the object
  disagreed about what the entry was.

- **`crypto.getRandomValues` reports a non-integer view as `TypeMismatchError`.**
  A `Float32Array` is the wrong *kind* of typed array, which the standard
  reports as that `DOMException`; it was a bare `TypeError`, indistinguishable
  from passing something that is not a view at all (which still is one).

- **`runtime:net` validates ports, and reports a bind like a connect.** A port
  that is not a port — negative, `NaN`, out of range, or missing — was coerced
  to `0`, so a typo'd port silently connected somewhere else instead of saying
  so. It is now a `SocketError` at the call. A bind failure reaching
  `Listener.addr` also carries the documented `TypeError: SocketError: …` shape
  rather than the raw host `Error`, so `listen` and `connect` no longer report
  the same class of problem two different ways. `listen({ port: 0 })` still
  means "pick an ephemeral port".

- **Runtime internals are no longer enumerable on `globalThis`.**
  `Object.keys(globalThis)` and `for…in` listed `__wasm_pending`,
  `__structuredSerialize`, `__wasm_module`, `__structuredDeserialize` and
  `__responseTrailers` beside `fetch` and `console`. They stay writable — the
  engine reinstalls them per isolate, and locking them would break snapshot
  restore — so this is presentation, not protection; authority lives in the
  Rust op table either way.

- **`process.args` reports as frozen.** It is frozen once seeded, but
  `Object.isFrozen` asks `[[IsExtensible]]` first, which had no proxy trap and
  so answered from the unseeded, still-empty array — making the documented
  "**Frozen**" read as false.

- **A WebSocket close reports the code the caller asked for.** A client calling
  `close(4001, "bye")` was told `1006` / `wasClean: false` by its own `close`
  handler, while the peer correctly received 4001 and the reason. 1006 means
  "connection dropped without a close frame" and must never mark a clean
  shutdown, so reconnect logic keyed on the code took the failure branch on
  every ordinary close. The end of the stream after a close *we* asked for is
  now reported as that close (1005 when no code was supplied, matching what the
  peer sees); an end of stream nobody asked for is still 1006 with an `error`
  before it.

- **A `runtime:http` handler that returns a non-`Response` is a 500.** It was
  coerced with `String(value)` and sent as a **200**, so `return { ok: true }`
  shipped the body `[object Object]` as a successful response — the documented
  behaviour has always been a 500. Both this and a thrown handler now also
  *report* the reason (to stderr, via the usual error path) instead of failing
  silently; the response itself stays a bare `Internal Server Error`, since a
  handler's mistake is not the client's business.

- **`fetch` rejects network failures with `TypeError`.** Connection refused, an
  unsupported scheme, a DNS failure and a redirect loop all rejected with a
  plain `Error`, so `catch (e) { if (e instanceof TypeError) … }` — the
  documented way to tell a transport failure from a programming mistake — never
  matched, while `redirect: "error"` on the same call did. The stable `code`
  (`ERR_TOO_MANY_REDIRECTS` and friends) survives the change. Aborts
  (`AbortError`/`TimeoutError`) and capability denials (`NotAllowedError`) are
  deliberately *not* network errors and keep their own types.

- **MessagePack carries binary data again.** `encode` wrote `nil` for a
  `Uint8Array` and `decode` returned a plain `Array` of numbers for a `bin`
  value, so binary — the reason to choose the format — was destroyed in both
  directions, silently and in full. Both paths now handle the `bin` family
  directly: a typed-array view or `ArrayBuffer` encodes as `bin` and decodes
  back to a `Uint8Array`, and an `ext` value keeps its payload bytes.

  The cause was the JSON pivot the module uses everywhere (parse to a JSON
  string, let the guest's `JSON.parse` build the graph — measurably faster than
  marshaling a value tree). JSON has no byte string. That pivot is kept for
  documents that *are* JSON-shaped, which is the common case; a document
  carrying `bin`/`ext` is detected by an exact structural scan that allocates
  nothing, and only that document pays for a value tree.

  Values with no own enumerable properties crossed the boundary as `{}` and
  encoded as an empty map — every entry gone. A `Map` now encodes as a map, a
  `Set` as an array, and a `Date` as its ISO-8601 string (so a `Date`
  round-trips as a *string*, not a `Date`). A value with no representation at
  all — a function, a symbol, a `BigInt` — now throws a `TypeError` instead of
  being written as `nil`.

- **WebCrypto key usages are enforced.** `sign`, `verify`, `encrypt`, `decrypt`,
  `deriveBits` and `deriveKey` ignored `key.usages` entirely: a key imported for
  `["verify"]` would sign, an encrypt-only key would decrypt. They now throw
  `InvalidAccessError`, as the standard requires. `importKey`/`generateKey`
  likewise reject usages the algorithm does not register, and a secret or
  private key created with no usages at all, with `SyntaxError` — recording a
  usage an algorithm cannot honour made `key.usages` meaningless as the
  authority record every later operation is checked against.

  Two things deliberately do *not* change: `deriveKey` is gated on `deriveKey`
  rather than on the `deriveBits` it uses internally (and `wrapKey` on `wrapKey`
  rather than `encrypt`), so a narrowly-granted key still works; and an
  algorithm that registers no such operation — `encrypt` on AES-KW — remains
  `NotSupportedError`, because the standard normalizes the algorithm before it
  looks at the key.

- **ECDSA works with every hash on every curve.** A digest narrower than the
  curve's field made signing fail outright with `OperationError`: P-521 with
  SHA-256, P-384 with SHA-1, P-521 with SHA-1. The backend refuses to widen a
  prehash under half the field width, so the padding SEC1's bits2int implies is
  now done before the call. Signatures cross-verify with other implementations
  in both directions; wider digests are untouched.

- **`setTimeout` no longer hangs the process on an over-range delay.** A delay
  past the 32-bit signed millisecond ceiling was scheduled verbatim, so
  `setTimeout(fn, 2 ** 40)` armed a timer no program outlives — and a pending
  timer is pending work, so the process never exited. Over-range delays now
  clamp to 1 ms, as Node and browsers both do.

- **`runtime:fs` no longer treats the root jail as a target.** `Path::join("")`
  is the path itself, so an empty path argument silently *became* the jail root
  and the operation ran against it: `remove("", { recursive: true })` deleted the
  entire project directory, `chmod("", 0)` locked it to mode `000`, and
  `rename("", "")` succeeded. Two guards, both at the jail — so `runtime:wasi`
  and any direct op call are covered on the same terms as `runtime:fs`:

  - An **empty path** is refused outright, for reads as well as writes. No
    operation intends it, and Node's `fs` rejects it too.
  - A **mutation whose resolved target is the root** is refused however it is
    spelled — `.`, `./`, `data/..`, or the root's own absolute path. Removing,
    renaming, truncating or `chmod`ing the root destroys the sandbox the guest
    is running in and is never a coherent request from inside it.

  Reads of the root are unaffected (`stat(".")`, `readDir(".")`, `realPath(".")`
  work as before), as are writes to entries *inside* it — including the temp
  entries `makeTempDir`/`makeTempFile` create there by default. Both failures
  carry the new `ERR_INVALID_PATH` code.

- **`Request.clone()` and `new Request(otherRequest)` carry the body.** The
  `Request` constructor never copied the body from an input `Request`, so both
  produced a request with no body at all — and since `clone()` *is*
  `new Request(this)`, a cloned request was always empty. `fetch(new
  Request(url, { method: "POST", body }))` sent nothing, which is the form
  middleware and request-rewriting code is written in. `init.body` still wins
  when given.

- **`Response.clone()` tees a stream-backed body instead of sharing it.** The
  clone took a shallow copy of the body state, so both halves pointed at one
  stream and reading either then the other threw `TypeError: stream is already
  locked`. Every `fetch` response is stream-backed, so the everyday `const r =
  await fetch(u); const c = r.clone();` was the failing case. Byte-backed bodies
  still share their (immutable) bytes rather than paying for a tee. Cloning a
  body that is already consumed or locked is now a `TypeError`, as the Fetch
  standard requires, rather than producing a broken clone.

## [0.18.0] - 2026-08-07

### Added

- **A Web Platform Tests subset for workers** (`wpt/`), running the upstream
  tests unmodified against a pinned, sparse checkout: `workers/`,
  `webmessaging/` and `html/webappapis/structured-clone/`. Every test runs
  **twice** — once on the agent driving the process, once inside a real
  dedicated worker — which is the first executable coverage the worker global
  scope has had; the curated `conformance/*.js` suite runs entirely on the
  driver agent.

  Baseline: **560 / 570 runnable subtests (98.2%)** across 70 completed runs,
  recorded per subtest in `wpt/expectations.json` and enforced as a floor, with
  newly-passing subtests reported so a fix cannot land without updating the
  record.

  What counts as *runnable* is decided by `wpt/scope.js`, which excludes — with
  a reason each, and only for things inapplicable by design — 18 files and 48
  subtests that test a renderer, a document, browser-local storage or classic
  scripts. Nothing is excluded for merely being unimplemented, so the failing
  count is exactly the work left.

  It is not yet a CI gate: the runtime defects it found are listed in
  `wpt/README.md`, and one of them (a terminated worker orphaning its own
  workers) means a full run does not exit on its own.

- **`--max-heap=<mb>`, and a heap that is no longer fixed at 256 MiB.** Every
  `esrun` agent was capped at the embeddable library default regardless of the
  machine — about a sixteenth of what Node (4288 MiB) and Deno (4192 MiB) give
  the same script on a 16 GiB host, with no flag to raise it.

  `esrun` now sizes the heap from the machine, as they do, and reads the
  **cgroup** limit before physical memory. Node and Deno both read physical
  memory here, which is why deploying either one means hardcoding
  `--max-old-space-size`: in a 2 GiB container on a 64 GiB host they size for
  64 GiB and get OOM-killed where a garbage collection would have done.

  ```
  esrun --max-heap=512 app.js    # pin it
  esrun app.js                   # container limit, else host memory
  ```

  The embeddable library is unchanged and deliberately so: `Limits::default()`
  is still a fixed 256 MiB, because a library that is one part of somebody
  else's process must not decide how much of it to take. `Limits::heap_limit_bytes`
  is now `Option<usize>`, with `None` meaning "size it from the host"
  (`Limits::with_system_heap_limit()`).

- **`new Worker(url, { memory })`** — a per-worker heap ceiling, in megabytes,
  as Node's `resourceLimits.maxOldGenerationSizeMb` is. Deno and Bun have no
  per-worker limit at all, so a runaway job there takes the whole process.

  ```js
  new Worker(url, { memory: 64 });
  ```

  Exceeding it ends that worker and no other, and the parent hears why:

  ```js
  w.onerror = (e) => {
    e.error.name  // "ERR_WORKER_OUT_OF_MEMORY"
    e.message     // "worker terminated: it reached its 64MB memory limit"
  };
  ```

### Changed

- **A worker's limits now derive from its parent's instead of being fresh.**
  `worker_limits()` built a `Limits::default()` per worker, so an embedder that
  had tightened its own runtime — say a 32 MiB heap — handed out 256 MiB agents
  to anything holding `workers`: the ceiling was escaped simply by doing the
  work in a worker. A worker now inherits the ceiling of the agent that started
  it, and `{ memory }` may only lower it, which is the rule `permissions` and
  `env` already followed. `Runtime::limits()` exposes what an agent was built
  with.

- **A worker that ran out of memory said "engine internal error".** V8 refuses
  to begin evaluating once an isolate is terminating, but its own terminating
  flag has usually cleared by the time it says so, so the heap-guard kill fell
  through to `Error::Internal("module evaluation failed to start")` — and a
  worker that hit the ceiling *after* startup looked to its parent like one that
  had simply finished. `Engine::heap_limit_exceeded()` (and
  `Runtime::heap_limit_exceeded()`) report the guard's latch, which is the only
  thing that distinguishes running out of memory from a watchdog, a
  `process.exit()` or a `terminate()`.

- **A curated `conformance/workers.js`** — 12 tests for the `Worker`
  constructor's contract and the surface a `Worker` object has. The suite is the
  pre-1.0 signal and had no worker file at all; it is now 343 assertions across
  24 files.

- **A fake `WorkerHost` in the runtime's own tests.** The worker seam could only
  be exercised end-to-end, which costs an OS thread and a V8 isolate per case
  and can observe nothing but what a worker prints. Five tests now assert what
  the runtime *asks the host to do* — the narrowed `WorkerSpec`, the order
  messages reach it, that `"inherit"` is bounded by the parent, that a reported
  failure reaches `onerror` with its class rebuilt — which is also what an
  embedder's own `WorkerHost` will be called with.

- **`worker.queued` and `self.queued`** — how many messages have been posted and
  not yet taken, in each direction.

  The only backpressure signal there is, and deliberately advisory: nothing
  refuses a message, so a producer that outruns its worker has to choose to pace
  itself.

  ```js
  for (const job of jobs) {
    w.postMessage(job);
    if (w.queued > 1000) await drain();
  }
  ```

  No other runtime exposes this — Node, Deno and Bun all queue without limit and
  give you nothing to look at.

- **`worker.unref()` and `worker.ref()`** — Node's handle ref-counting, which
  Bun also has and Deno does not.

  A live worker is a reason for the process to keep running, which is right
  until a pool holds four idle ones waiting for the next job — then it is the
  reason the process never exits.

  ```js
  const w = new Worker(url);
  w.unref();   // still running, still delivering; no longer a reason to stay up
  w.ref();     // back to keeping the process alive
  ```

  The claim is a count the agent holds rather than the pending receive's own
  keep-alive flag. The receive cannot be taken back: an idle worker's is already
  in flight, so flipping its flag would only take effect on the next message,
  and for an idle worker there is no next message.

- **`new Worker(url, { permissions: "inherit" })`** — the parent's whole set, in
  one word, spelled the way `env` already spells it.

  Omitting `permissions` still grants **nothing**, which is the difference from
  `env` and is deliberate: passing data is not granting authority, since a
  parent can only hand over values it could already read, whereas a capability
  it did not name is one it did not mean to give. `"inherit"` is a ceiling, not
  an escape — the host still intersects it with the spawning agent's own set, so
  an inheriting worker under `--deny-net` is denied net too.

- **`new Worker(url, { permissions: ["nett"] })` silently granted nothing.** An
  unrecognised name was skipped. Fail-closed, and therefore quiet: the worker
  took the degraded path forever and the denial surfaced three layers from the
  typo. A non-array value (`permissions: "net"`) was ignored outright.

  Both now throw a `TypeError` from the constructor, where the other malformed
  options already throw:

  ```
  unknown Worker permission "nett" — expected one of: read, write, imports,
  net, listen, env, run, signals, workers
  ```

  This is the rule `runtime:process` `permissions.has()` has always followed;
  the constructor was the one place not following it. Deno accepts an unknown
  name in `deno: { permissions }` silently. (`esrun`'s own `--deny-<name>` flags
  already rejected typos with the same list.)

  A new ungated `permission_names` op serves the authoritative vocabulary
  (`Capability::HOST_FACING`) to both JS readers, so `runtime:process` drops the
  hand-transcribed copy it carried.

- **TypeScript coverage for the worker options.** `permissions` was typed
  `readonly string[]`, so an editor caught no typo either; it is now
  `"inherit" | readonly PermissionName[]`, sharing the union `runtime:process`
  already exports. `memory` was missing from `types/` entirely — added, with the
  narrowing rule and `ERR_WORKER_OUT_OF_MEMORY` documented. Two tests keep the
  definitions honest: the `PermissionName` union is checked against
  `Capability::HOST_FACING`, and every non-standard `WorkerOptions` member must
  be declared.

- **A denied import now says which import, and where the grant is made.** The
  refusal was `capability denied: FileSystem (permission "imports")` — an
  internal capability, a permission the author may never have mentioned, and no
  clue which of the file's imports failed.

  It matters most inside a worker, where the advice differs: a worker's grants
  are set at the spawn, in the parent, so `--allow-imports` is the flag for the
  wrong agent.

  ```
  in a worker:   cannot import "./dep.mjs": this worker was not granted the
                 "imports" permission — grant it at the spawn,
                 new Worker(url, { permissions: ["imports"] })
  otherwise:     cannot import "./dep.mjs": the "imports" permission is not
                 granted — add --allow-imports
  ```

  This is the one a worker actually hits, because static and dynamic imports are
  not the same operation: a worker's static graph is resolved by its parent up
  front, so `import` works where `import()` does not. That asymmetry is
  deliberate — `import()` picks its specifier at runtime and so reads *and
  executes* a file chosen while the worker runs — and is now documented rather
  than discovered.

- **A denied dynamic `import()` now rejects with the exception it always
  should have.** `reject_dynamic_import` hand-rolled the rejection from four
  builtin classes and flattened everything else to a plain `Error`, so a
  capability refusal arrived as `Error` with no `code` and could only be
  recognised by matching its text. It now builds the exception the same way
  every other Rust error crossing into JS does:

  ```js
  try { await import("./x.mjs"); }
  catch (e) {
    e.name  // "NotAllowedError"        (was: "Error")
    e.code  // "ERR_CAPABILITY_DENIED"  (was: undefined)
  }
  ```

  A syntax error still rejects with a `SyntaxError`, as before. Embedders:
  `Engine::reject_dynamic_import` now takes `&dyn IntoException` in place of
  `(ExceptionClass, &str)`.

- **A remote module specifier said `npm install`.** `import("https://…")` fell
  through the `node_modules` walk and reported `cannot find package "https:" …
  run npm install?`, sending the reader after something no install could fix.
  Remote modules are a stated non-goal, and the message now says so. (The
  fall-through happened whenever the URL did not parse — a well-formed one was
  already reported correctly.)

- **A refused spawn now names the flag that fixes it.** Starting a worker takes
  `imports` as well as `workers` — the parent reads the worker's entry module,
  and reading a module is what `imports` grants — so `--deny-all
  --allow-workers` was refused with `capability denied: FileSystem (permission
  "imports")`: an internal capability and a permission the author never
  mentioned, with nothing connecting either to the `new Worker()` that failed.

  ```
  cannot start a worker from ./w.mjs: reading its module needs the "imports"
  permission — add --allow-imports (capability denied: FileSystem)
  ```

  Context only — the capability gate in the host is still the whole enforcement,
  and `e.error.name` (`NotAllowedError`) and `e.error.code`
  (`ERR_CAPABILITY_DENIED`) are untouched, so anything branching on the refusal
  is unaffected. Node requires `--allow-fs-read` alongside `--allow-worker` for
  the same reason and answers `ERR_ACCESS_DENIED`; Deno requires `--allow-read`
  and names the flag, which is the behaviour copied here.

  `DECISIONS.md` D49 claimed `esrun --deny-all --allow-workers app.js` worked.
  It never has; the record now carries the command that does.

- **A worker's failure now arrives in pieces, not as one formatted string.** The
  parent's `error` event carried the whole stack in `message`, with `filename`,
  `lineno`, `colno` and `error` all left at their empty defaults — so the only
  way back to *which* error had failed was substring matching, which is the one
  thing a supervisor needs before it decides whether to retry.

  ```js
  worker.onerror = (e) => {
    e.message                     // "out of range"     (was: the whole stack)
    e.filename                    // "file:///app/job.js"
    e.lineno; e.colno             // 2; 34
    e.error instanceof RangeError // true — with the worker's own .stack
  };
  ```

  The failure crosses a thread boundary, so `e.error` is necessarily a rebuilt
  object; it is rebuilt as the class it was thrown as when that class is a
  standard one, and otherwise as an `Error` carrying the right `name` — which is
  the discriminator that survives anyway, since a `DOMException` is told apart by
  `"AbortError"` rather than by its constructor.

  Node reconstructs the error but has no location fields at all (its
  `worker.on("error")` hands over an `Error`, not an `ErrorEvent`); Deno fills
  the location fields but leaves `e.error` null; Bun leaves both empty. This
  fills both.

  A rejection reason is no longer re-worded as `"unhandled rejection: …"` on the
  way, for the same reason: `e.error.name === "RangeError"` says more than the
  prefix did.

  For embedders: `TickStatus.unhandled_rejections`, `TickStatus.uncaught_errors`,
  `DriveOutcome`, `DriveFailure`, `ModuleEvalState::Failed` and
  `WorkerScope::report_error` all carry the new `es_runtime_common::UncaughtError`
  instead of a `String`. Its `Display` renders exactly what the `String` did, so
  code that only prints a failure needs a `.to_string()` and nothing more.

- **A worker that fails now says so at once, and stops.** An uncaught error or
  unhandled rejection inside a *running* worker was collected into the drive's
  outcome and reported only when the worker ended — so a parent that terminated
  it never heard about the failure at all, and one that waited heard far too
  late to retry anything. A throw inside `onmessage` never reached the parent by
  any route, because `dispatchEvent` catches a listener's exception and reports
  it through `reportError`, which never left the worker's own agent.

  Each unclaimed failure now reaches the parent's `onerror` the tick it happens,
  and ends the worker: an exception that escaped every handler its author wrote
  leaves the agent in a state nobody can vouch for, so a supervisor gets one
  clean transition to restart on rather than an agent that stays in the rotation.
  Node, Deno and Bun all end the worker here too.

  A worker that takes responsibility is unaffected — `preventDefault()` in its
  own `error` or `unhandledrejection` listener claims the failure, which is
  neither reported nor fatal:

  ```js
  self.addEventListener("error", (e) => {
    postMessage({ failed: currentJob, reason: e.message });
    e.preventDefault();          // absorbed; this worker keeps its next job
  });
  ```

  A worker that merely *hears* about a child worker's failure has not failed
  itself, so an unclaimed `error` on a `Worker` object is written to the console
  rather than escalated — without that, one leaf failure would take down every
  ancestor that had not attached an `onerror`.

### Fixed

- **A WPT run wasted 50 seconds on four dead files.** Two
  `webmessaging/message-channels/worker*.any.js` files build their worker from a
  `blob:` URL and set no `onerror`, so the failure reached nothing and each mode
  waited out its ten-second deadline — reported as "no result before the
  deadline" when the cause was known and permanent. They are excluded in
  `scope.js` now, with the distinction spelled out: a test *of* blob:/data:
  worker URLs stays a counted failure, but one whose subject is `MessagePort`
  and that merely builds a fixture that way cannot run here at all. A full run
  is ~11s instead of ~51s, with the same 560/570.

- **`postMessage` could throw, and take the whole agent with it.** `Worker`'s
  `postMessage` and a worker's own were registered as **async** ops, so every
  send held one of the agent's `max_pending_ops` slots. A burst of about 1150 in
  one turn — posting 2000 jobs in a loop, say — threw
  `RangeError: too many concurrent async operations`, and from then on *every*
  async op in that agent failed: `terminate()`, `fetch`, a timer, an fs read.
  One noisy producer broke everything.

  ```js
  await new Promise((r) => setTimeout(r, 300));
  for (let n = 0; ; n++) w.postMessage(n);
  // before: threw after 1150 posts; w.terminate() then threw too
  // after:  5000 posts, then a clean terminate
  ```

  A queue push has nothing to wait for, so both are now synchronous ops over
  synchronous `WorkerHost::post` / `WorkerScope::post` — which is what
  `PortHub::post` and `MessagePort.postMessage` always were, and why they never
  had this. HTML does not permit `postMessage` to fail for queue depth, and no
  other runtime does.

- **A parent worker's child list grew forever.** `Live::children` was appended on
  every spawn and never pruned, so a supervisor that starts one worker per job
  held an id for every job it had ever run — 8 bytes each, for the life of the
  process — and walked all of them on each `terminate()`. Retiring a worker now
  unhooks it from its parent as well as removing it from the registry.

  Never a correctness bug: ids only count up, so a stale entry could not name a
  later worker. That is what made it a leak rather than a fault, and what let it
  go unnoticed. `default-providers`'s worker host had no unit tests at all; it
  now has four, covering exactly this bookkeeping.

- **`WorkerHost::has_live_workers` and `WorkerHost::shutdown` are gone.** Nothing
  ever called either, and the trait's own documentation was wrong about one of
  them: "the embedder's loop reads this to decide whether the process still has
  work" — it does not, and never did. What keeps a process alive for a live
  worker is the outstanding receive (now the reference count above). Embedders
  implementing `WorkerHost` had to write both for nothing.

- **`Worker.postMessage()` could deliver messages out of order.** A dedicated
  worker's port is entangled when the constructor returns, so HTML has no window
  in which posting order can be lost — but this runtime's spawn is asynchronous
  (the entry is read, then the agent starts), so messages posted meanwhile are
  queued in the `Worker` and flushed once there is an id to send them to. That
  flush awaited each message, and the id was already set, so a `postMessage` from
  a microtask took the direct path and overtook messages still waiting:

  ```js
  const w = new Worker(url);
  for (let i = 0; i < 5; i++) w.postMessage(i);            // queued
  Promise.resolve().then().then().then(() => {
    for (let i = 10; i < 15; i++) w.postMessage(i);        // overtook 1..4
  });
  // the worker received 0,10,11,12,13,14,1,2,3,4,…
  ```

  The queue is now flushed synchronously, so nothing runs between the id
  becoming observable and the queue being empty. Node, Deno and Bun all deliver
  this in order; now so do we.

- **`permissions.has("workers")` threw instead of answering.** The capability
  arrived with workers (D48) and the introspection list was left at eight names,
  so the supported way to ask got `TypeError: 'workers' is not a permission
  name` — while `permissions.denied` listed it, since that comes from the Rust
  side. Nine names now, everywhere: the flag, the `Worker` option, `denied`,
  `has()` and `PermissionName`.

- **A transferred `MessagePort` never reached the receiver.** A port named in a
  transfer list but not referenced by the message itself was validated,
  detached, and then dropped: its queue was handed to a receiver that never saw
  the port. `event.ports` was empty in every case — which is the *only* way the
  spec hands a transferred port over:

  ```js
  channel.port1.postMessage("here", [other.port1]);   // event.ports was []
  ```

  A `postMessage` now carries its transferables with the message.
  `structuredClone` is unchanged: what it transfers, it transfers into the value
  it returns.

- **`MessageEvent.ports` is a frozen array**, as WebIDL's `FrozenArray` requires.

- **Messaging refusals the spec asks for**, each previously silent: transferring
  a port through *itself*, transferring a port that has been closed, and
  transferring one that was already transferred away. All three now raise
  `DataCloneError`.

- **`postMessage()` with no arguments now throws** on `MessagePort` and
  `BroadcastChannel`, instead of sending `undefined`.

- **Events the platform fires report `isTrusted: true`.** It was hard-coded
  `false`, so a listener could not tell a delivered message from one any script
  could dispatch — which is the attribute's only purpose.

- **A platform object with no serialization now refuses to be cloned.**
  `structuredClone(new Response())` walked it as an ordinary object and returned
  `{}` — a clone that quietly threw the value away. `Response`, `Request`,
  `Headers` and `FormData` now raise `DataCloneError` naming the interface.

- **Transferring no longer depends on the interface still being global.**
  `delete globalThis.MessagePort` made a port untransferable, because the check
  was `instanceof globalThis.MessagePort`; the spec's algorithm never consults
  the global, and now neither does this.

- **An event handler attribute keeps a non-callable object.** `onmessage = {…}`
  stored `null`; WebIDL's `[LegacyTreatNonObjectAsNull]` keeps the object (the
  getter returns it) and simply never invokes it. Only a non-object becomes
  null.

- **`BroadcastChannel` delivery order.** Two faults: `new BroadcastChannel()`
  subscribed *asynchronously*, so a channel missed anything posted before its
  subscription resolved — the spec joins the channel as the constructor runs —
  and the hub iterated a `HashMap`, so a post reached the other channels in
  whatever order the hasher gave rather than the order they were created.

  Delivery is now one ordered stream per agent instead of one receive per
  channel, which is what makes every destination of one post arrive before any
  destination of the next, exactly as the spec's single task queue does.

### Changed

- **`blob:` URLs across agents, and `blob:`/`data:` worker URLs, are now stated
  non-goals** rather than deferrals (SPEC §14, DECISIONS D48). Both schemes
  exist to carry code and data around inside a page; on a server the file is
  already on disk and the bytes already cross by `postMessage`. Behaviour is
  unchanged — `new Worker("data:…")` is refused with the scheme named, and a
  `blob:` URL still resolves on the agent that minted it. `FileReader` and
  `EventSource` are likewise declined: `Blob.text()`/`.arrayBuffer()`/`.stream()`
  supersede the first, and the second is a client for a protocol a server
  implements rather than consumes.

### Added

- **`new Worker(url, { env })`** — the environment a worker reports from
  `runtime:process`, either `"inherit"` (the default: the host environment,
  still needing the `env` permission and still narrowed by `--allow-env`) or an
  object of variables:

  ```js
  new Worker(url, { env: { DATABASE_URL: unmask(env.DATABASE_URL) } });
  ```

  A handed environment needs **no permission**, because nothing is granted: a
  parent can only pass values it could already read, so this attenuates — the
  same move `permissions` makes, applied to data rather than authority. It is
  also the only way to say "this variable and no other", since `--allow-env` is
  set by the deployment rather than at the spawn.

  A handed environment wins over the host's, `{}` is a worker with no
  environment, and secret-looking names are re-masked on arrival — so a `Secret`
  can be passed straight through and stays one. Node's `SHARE_ENV` has no
  equivalent, deliberately (DECISIONS D49).

- **A worker's global scope is now a real `DedicatedWorkerGlobalScope`.** The
  members were always there, but the interfaces behind them were not, so the
  one question the platform answers by them — *am I in a worker?* — could not
  be asked:

  ```js
  if ("DedicatedWorkerGlobalScope" in self && self instanceof DedicatedWorkerGlobalScope) {
  ```

  which is the idiom HTML intends, Deno supports, and WPT's own helpers use. A
  worker whose scope did not answer it silently took no branch at all.

  `WorkerGlobalScope` and `DedicatedWorkerGlobalScope` are exposed in a worker
  (and only there), the members move onto their prototypes, and the global's
  prototype chain becomes `DedicatedWorkerGlobalScope` → `WorkerGlobalScope` →
  `EventTarget`. `self` becomes the interface's readonly attribute, so it can no
  longer be overwritten.

- **`navigator` in a worker is a `WorkerNavigator`**, and `Navigator` is no
  longer exposed there. One interface per scope, as the spec has it; the members
  are unchanged, and are built from `Navigator`'s own descriptors so the two
  cannot drift.

- **`location` in a worker**, a read-only `WorkerLocation` over the worker's own
  module URL — so `new URL("./data.bin", location)` resolves a sibling file. The
  agent driving the process still has none: no one script there is *the* script.
  The same set Deno exposes in a module worker.

### Fixed

- **`exit()` hung the process unless it was the last statement to run.** It
  terminates the isolate, and a termination unwinds whatever was running without
  settling it — so a module suspended at a top-level `await` stayed pending
  forever, and the loop waited on work that could never finish:

  ```js
  await null;
  exit(0);
  console.log("unreachable");   // never runs, and the process never exits
  ```

  Every ordinary shape was affected — an early exit from a loop, a guard clause,
  a timer callback — and it reproduces back to 0.13.0. The driver now stops when
  execution has been terminated, which also covers the watchdog and the heap
  guard.

  Finding it needed one more step than expected: V8's own "is terminating" flag
  answers a narrower question than it appears to, reporting only whether
  *currently running* JavaScript is unwinding. `exit()` at the end of a timer
  callback sets the request, the callback then returns normally with nothing
  left to reach an interrupt check, and by the time the loop looks there is
  nothing to see. `InterruptHandle` now latches the request.

- **A dynamic `import()` waited for an unrelated timer.** A linked import's
  promise is settled by the *next* tick, and the driver parked before taking it
  — so the import inherited whatever the loop was about to park on:

  ```js
  setTimeout(() => {}, 3000);
  await import("./x.js");       // resolved after 3000 ms, not 3 ms
  ```

  With nothing else pending it took the loop's fallback park instead. Any lazily
  imported module in a program that also has timers or I/O in flight paid this.
  The driver now re-ticks immediately after linking, the same rule it already
  applied to V8's background compilation.

- **`terminate()` left a worker's own workers running.** HTML terminates the
  nested ones along with the parent; here they were orphaned — unreachable,
  since the agent holding them was gone, and still keeping the process alive,
  since a live worker is a reason not to exit. A worker that started a worker
  could therefore make a program that never terminates.

  The host now tracks which agent started which (the calling thread identifies
  the calling agent — one agent per thread is the whole model) and terminates
  the subtree. Every member is told to stop before any of them is joined, which
  the ordering demands: a parent's loop holds an outstanding receive on each
  child, so joining the parent first waits for a child that has not been asked
  to stop.

- **`close()` did not end a worker that had other work outstanding.** It turns
  back the receive pump, so an agent holding any other pending op — a worker
  waiting on a worker of its own, most obviously — carried on driving forever.
  HTML has `close()` discard the remaining tasks; the drive now stops with it.

- **`runtime:fs` `write()` could resolve over a file that was not written yet.**
  Above 64 KiB the write takes an async path, and `tokio::fs::File` dispatches
  writes to a blocking pool: `write_all` returned before they landed and the
  file was dropped without a flush. Since the same call had already truncated
  the file, a read straight afterwards saw *less* than before the write —

  ```js
  await write("big.json", payload);      // 260 KB
  await file("big.json").text();         // 0 bytes, or a prefix
  ```

  — in 18 of 25 attempts at 260 KB. It now flushes before resolving, which is
  what the `FileSystem::write` contract always claimed. Under 64 KiB is a
  synchronous `std::fs` write and was never affected.

- **`Atomics.wait` on the main thread hung the process.** ECMAScript gates the
  call on the agent record's `[[CanBlock]]`, and HTML sets that `false` on the
  agent that drives the loop — so the spec-required answer is a `TypeError`. We
  never set it, and V8 defaults to allowing the call, so

  ```js
  Atomics.wait(new Int32Array(new SharedArrayBuffer(8)), 0, 0);
  ```

  parked the only thread that can make progress: no timers fired, no async op
  settled, no interrupt was delivered. The process hung until `--timeout`
  terminated it, and with no `--timeout` it hung forever.

  `Limits` gains `can_block` (default `false`), wired to V8's
  `SetAllowAtomicsWait`. The call now throws, as it does in browsers, Deno and
  Bun. A worker agent will set it `true` — blocking there stops only its own
  thread, which is what `Atomics.wait` is for.

- **`structuredClone` rejected ordinary objects with a class prototype.**
  `structuredClone(new Foo())` threw `DataCloneError`, where the spec serializes
  an object's own enumerable String-keyed properties and rebuilds a plain
  object — what browsers, Node, Deno and Bun all do. The hand-written clone
  refused any prototype other than `Object.prototype`/`null`.

- **`structuredClone` copied symbol-keyed properties.** StructuredSerialize
  walks String keys only; the clone used `Reflect.ownKeys`, so an enumerable
  symbol-keyed property came along.

- **`structuredClone(view, { transfer: [view.buffer] })` threw.** The spec
  detaches *after* serializing, so a view over a buffer in the transfer list is
  serialized while the buffer is still live: the clone carries the data and the
  source is left detached. It now does. An ArrayBuffer that is *already*
  detached on the way in is still refused, and now as the `DataCloneError` the
  spec asks for rather than a `TypeError`.

### Changed

- **A worker may start its own workers.** The spec allows nesting, and what
  bounds it is the capability chain — a worker can only spawn if it holds
  `workers`, and can only pass on what it holds — rather than withholding the
  constructor.

- **`onSignal` is refused inside a worker.** A signal is delivered to the
  process and watching one suppresses the default action, so a worker taking
  `SIGTERM` would decide, from a thread the program may not know is running,
  whether the process declines to die. Node reaches the same conclusion.

- **`exit()` inside a worker ends that worker, not the program.** Halting was
  already per-agent, but the exit *code* was recorded on a shared provider, so
  a worker could set what the whole process exited with.

- **A started `MessagePort` delivers its queue asynchronously**, as the spec's
  task-based delivery requires. `start()` used to flush the buffer inline, so a
  handler had already run by the time it returned; it now runs a turn later, as
  it does in a browser.

- **`structuredClone` is now HTML's StructuredSerialize/StructuredDeserialize**,
  performed by the engine over V8's `ValueSerializer`, replacing the
  hand-written JS deep clone. That is where the three fixes above come from.

  It changed because workers need a serialized form that can cross an isolate,
  and keeping the JS clone alongside it would have meant two implementations of
  one algorithm — drifting, so that a value cloning fine through
  `structuredClone` failed through `postMessage`.

  `Blob`, `File` and `DOMException` are host objects V8 has no representation
  for; they register a codec beside their own definitions, reached through the
  serializer's delegate. Everything previously supported still round-trips.

### Added

- **`Worker`** — the HTML dedicated worker, each with its own OS thread and its
  own V8 isolate. `postMessage`, `onmessage`/`onmessageerror`/`onerror`,
  `terminate()`; inside, a `DedicatedWorkerGlobalScope` with `self.postMessage`,
  `onmessage`, `close()` and `name`. Messages carry the full structured-clone
  type set — `Map`, `Set`, `Date`, `BigInt`, typed arrays, cycles — not JSON.

  `Worker` is **not** in the WinterTC Minimum Common API; this follows the HTML
  Standard, as Deno and Bun do. Module workers only: `type: "classic"` throws,
  because this runtime evaluates every input as a module (SPEC §8) — the same
  reason `require` is absent. Deno refuses them for the same reason.

  Spawning is a provider (`WorkerHost`), not something the runtime does itself:
  `runtime` still owns no thread and no loop. `ThreadWorkerHost` is the
  reference implementation.

  **A worker starts with no capabilities.** It is granted them explicitly, and
  only ones its parent already holds:

  ```js
  new Worker(new URL("./w.js", import.meta.url), { permissions: ["net"] })
  ```

  So no chain of spawns widens the original grant — a difference from Deno,
  which clones the parent's permissions unmodified. `--deny-workers` refuses the
  spawn outright. A worker's own static imports still load, under the parent's
  authority to read them, so deny-by-default does not mean single-file workers;
  that is safe because instantiation runs no guest code.

  A relative `new Worker("./w.js")` resolves against the entry module. Prefer
  `new URL("./w.js", import.meta.url)`, which is exact — and what Vite, webpack
  and Deno all recommend.

  A live worker keeps the process alive, as in Node and Deno; `close()` or
  `terminate()` ends it. `terminate()` interrupts the isolate, so it stops a
  worker spinning in a synchronous loop or parked in `Atomics.wait`.

- **`SharedArrayBuffer` crosses to a worker as one allocation**, not a copy:
  the backing store is handed over, so `Atomics` between two agents operate on
  the same memory. `API.md` said it "buys nothing here" because there was
  nothing to share with; there is now. A transferred `ArrayBuffer` still moves
  by value — the sender detaches, the receiver holds the data.

- **`BroadcastChannel` reaches every agent**, not just its own. The spec scopes
  it to the agent cluster, which was indistinguishable from "this isolate" while
  there was one agent; with workers it is not, and a channel that reached only
  its own agent would be wrong rather than merely limited. Delivery goes through
  the new `BroadcastHub` provider (`ProcessBroadcastHub` covers one process); an
  embedder that installs none keeps the previous agent-local behaviour. A
  channel still never receives its own posts.

- **A `MessagePort` can be transferred**, including into a worker — the HTML
  spec's composition primitive, and the way to hand a worker a private channel
  rather than routing everything through the `Worker` object. Port queues move
  to the new `PortHub` provider (`ProcessPortHub` covers one process), so
  transferring a port moves its id and leaves messages already in flight
  queued where they were. A port still cannot be *cloned*: two ends of a
  channel cannot become three, so a port outside the transfer list is a
  `DataCloneError`. With no hub installed, ports stay agent-local and
  transferring one is refused, as before.

- **Transferable streams** — `ReadableStream`, `WritableStream` and
  `TransformStream` in a transfer list, including into a worker. A stream is not
  copied: its chunks cross a `MessageChannel` as they are produced, which is
  what makes an endless stream transferable at all. Transferring locks the
  original, as the spec requires, and a locked stream cannot be transferred.

  Backpressure crosses with it — the reading agent asks for each chunk and the
  writing agent's `write()` does not settle until it does — so a fast producer
  cannot run away into the port's queue. Like a port, a stream may be
  transferred and may not be cloned.

- **`navigator.hardwareConcurrency`** — the number a worker pool is sized from,
  via the `Process` provider (`available_parallelism`, so a container sees its
  share rather than the whole machine). Ungated, like `platform` and `arch`: it
  describes the machine the guest already runs on.

- **StructuredSerialize/StructuredDeserialize in the engine**, over V8's
  `ValueSerializer`, as the `__structuredSerialize`/`__structuredDeserialize`
  builtins.

- `OpDecl::unref`, marking an async op that should not by itself keep the
  embedder's loop running — Node's `unref`. Used by the `MessagePort` and
  `BroadcastChannel` receive pumps.

- `Runtime::instantiate_module_source` / `Runtime::begin_evaluation`, the two
  halves of `load_module_source`. Instantiation runs no guest code, so an
  embedder can load a program under one capability set and evaluate it under a
  narrower one — which is how a worker's graph loads.

- `Driver::drive_while`, for advancing a runtime until a condition rather than
  to quiescence. An agent holding a pending op on purpose — a worker waiting on
  `onmessage` — never reaches quiescence, so a failure that must be observed
  while it runs cannot wait for the drive to return.

  It exists because an op cannot carry a JS object graph: op handlers receive
  the closed `Value` enum, so a `Map`, a cycle or a class instance arrives as
  its `String(value)` coercion. Serializing where the live value still exists
  keeps the object graph off the op boundary entirely — the op moves a
  `Vec<u8>` like every other byte-carrying op, and `Value` is unchanged.

  Host types (`Blob`, `MessagePort`, …) ride V8's delegate hooks, which call
  back into JS, so "what a `Blob` is" stays in the prelude and the engine stays
  web-agnostic. **The byte format is engine-specific and versioned** — valid
  only between isolates of the same engine build, never to be persisted or sent
  over a network.

- `Runtime::with_snapshot_and_limits`, for constructing an isolate with chosen
  `Limits` from a snapshot. The blob is now taken as `Cow<'static, [u8]>`, so an
  `include_bytes!` snapshot is shared across agents rather than copied per
  agent.

## [0.17.0] - 2026-08-05

### Fixed

- **The HTTP server leaked a disconnect watch on every request.** Each request
  inserted a `oneshot::Receiver` into the provider's `delivered` map, and only
  `request_disconnected` ever removed one — which the guest reaches solely by
  touching `request.signal`. A handler that never looks at the signal, which is
  the common one and every hello-world, therefore left an entry per request for
  the life of the server.

  Measured on the Hono benchmark: 50k requests → 47MB, 200k → 67MB, 500k →
  112MB, from an idle 25MB, and none of it came back. About 175 bytes a request.
  `respond` now drops the watch alongside the response sender — once the response
  is sent there is nothing left to report a disconnect to, and a handler that
  asked first has already taken the receiver.

  Peak RSS is now flat at 43MB across 50k, 200k and 500k requests. The published
  server figures move with it: Hono **223MB → 46MB** (the lowest of the four
  runtimes, under Bun's 50, Deno's 72 and Node's 128), static files **344MB →
  131MB**. `request.signal` still fires on client disconnect.

- **`esrun script.js | head` panicked.** Rust ignores `SIGPIPE`, so writing to a
  closed pipe returns `EPIPE`, and `println!` panics on it — turning an everyday
  shell idiom into a Rust backtrace on stderr and an exit code of 1. The console
  sink now treats a broken pipe on stdout as what it is, the reader having taken
  what it wanted, and exits 0 silently as Node and Deno do.

- **Benchmarks reported Bun as a release that does not exist.** `bun --version`
  on a `bun upgrade --canary` build prints the unreleased version it is working
  towards, so every published comparison named "bun 1.4.0" and read as though it
  had been measured against stable. `bun --revision` says
  `1.4.0-canary.1+095eb31ae`. Fixed in `run.sh` and in all three probe scripts,
  whose shared version helper had to stop stripping the suffix that says so.

### Changed

- **`TextDecoder.decode()` decodes UTF-8 by validating it.** The op built an
  `encoding_rs::Decoder` per call, allocated the encoding label per call, and
  transcoded into a buffer sized by `max_utf8_buffer_length` — up to three times
  the input, to produce bytes identical to its input. UTF-8 is what
  `new TextDecoder()` gives you, and for it `String::from_utf8` checks the buffer
  in place. Throughput **2.49 → 6.44 GB/s**; verified against Node byte for byte
  on windows-1252, multi-byte UTF-8, BOM handling, lossy replacement and `fatal`.

  Adds `encoding_large`, a 64 KiB round trip: the existing `encoding` row uses a
  20-character payload, so it measures per-call cost and could not see this.

- **Typed-array arguments are no longer zeroed before being overwritten.**
  `marshal` built each landing buffer with `vec![0u8; len]` and handed it to
  `copy_contents`, which overwrites every byte — a second full pass writing zeros
  nothing reads. A further 12% on a 64 KiB decode, and it helps every op handed a
  typed array.

- **The HTTP server shares its per-connection strings.** `peer_host` and
  `origin` are fixed for a connection's lifetime but were cloned into every
  request — two allocations and two copies to reproduce bytes that never change,
  and on HTTP/2 one connection can carry hundreds of requests. As `Arc<str>` the
  clone is a refcount bump. Hono **51,209 → 53,928 req/s**, about 5%.

- **Two tests asserted POSIX behaviour and failed on Windows.**
  `import.meta.resolve` of an absolute path resolves against the root of the base
  URL, which on Windows includes the drive letter — `file:///D:/abs/z.mjs` is
  WHATWG resolution and what Node prints there, not a defect. And `\` escaping in
  a glob is disabled by globset on Windows, where `\` is the path separator (the
  same call Node's minimatch makes with `windowsPathsNoEscape`), so `\!x.ts` is a
  path rather than a literal `!x.ts`. Both assertions now state the real
  behaviour, and the glob helper documents the platform difference it had
  promised away.

### Tooling

- **The benchmark gate refuses numbers measured on a different build.** v0.12
  through v0.15 all shipped the same data — identical to the digit — because it
  was never regenerated; `runtimes.esrun` said "esrun 0.9.0" on all four. The
  site described a build seven minor versions old and nothing compared the two.
  A mismatch between the measured version and the workspace version is now a
  rejection.

  This also retires a "15MB memory regression" that was never real: the `http`
  row's 46MB was measured on 0.9.0, and today's 61MB is 0.16.0.

## [0.16.0] - 2026-08-05

### Documentation

- **An internals tier for the docs.** `/docs/internals/http` explains what
  happens to a connection between `accept` and `close` — every limit and default
  with its reasoning, what a connection costs in memory, and a measured
  comparison against Node, Bun and Deno. The reference says *what*, the guides
  say *how*, and this says *why, and what it costs*; each fact has one home and
  the others link to it.

  It also covers the request handoff (why one isolate can answer many
  connections, and why HTTP/2 multiplexing needed no new machinery), how
  `request.signal` knows the client went away, and what draining actually waits
  for.

  `bench/probe-runtimes.sh` produces the cross-runtime numbers by standing up all
  four servers and probing them, then rewrites the table in the page. Numbers in
  documentation rot; these can be re-measured with one command.

  `/docs/internals/sockets` does the same for `runtime:net`: why every socket
  owns two spawned tasks (an op cannot poll a socket the event loop is waiting
  on), how `startTls` reclaims the stream halves back from those tasks without
  losing buffered bytes, why TLS trust anchors are bundled rather than read from
  the platform, and what a raw socket deliberately does not bound — including
  that a `connect()` in progress cannot be cancelled from guest code.

  `/docs/internals/fetch` covers the outbound direction: why the HTTP clients are
  built lazily and why there are two of them, what is negotiated on the wire, why
  the only timeout is on connect, how content codings are decoded (and why an
  unknown one passes through untouched), and that **every redirect hop is checked
  against the allowlist**, not just the URL the guest wrote.

  `/docs/internals/websockets` covers the actor task behind every connection,
  what the host answers without telling you (ping), why sends are coalesced into
  one write, how each way of closing actually completes, what now bounds a
  connection, and that there is still no keepalive and no backpressure a sender
  can feel.

  All four live under an **Internals** section in the docs sidebar, one page per
  subsystem.

### Fixed

- **Async WebAssembly compilation was paying a millisecond of park latency per
  compile.** V8 compiles on its own background threads and reports completion as
  a *foreground task*, which `tick` drains — but posting that task touches
  nothing the driver is parked on, unlike an op future, which signals the
  driver's waker. So the loop fell to its 1ms fallback sleep and charged it to
  every compile: 1.78ms each on the bench's 60-module row, against V8's own cost
  of about 0.6ms. `TickStatus` now reports V8 background work in flight and the
  driver yields instead of sleeping while it is.

  `wasm_compile` **143ms → 40ms**, from three times Deno's to level with it and
  ahead of Node — running the same compiler it always was. This was never a
  compiler gap; it was the loop declining to come back and look.

- **Every URL component setter re-parsed the whole URL.** The JS `URL` object
  holds nothing but its `href`, so `u.hostname = ...` parsed that string from
  scratch to apply one change — 0.44µs of parse against 0.56µs of actually doing
  the work, on a URL the previous call had just produced. The host now keeps a
  bounded cache of parsed URLs, and a setter puts its result back, so the next
  setter on the same object finds it already parsed. `url_setter` **263ms →
  183ms**, past Deno and Bun.

  Keyed by the URL's own serialization, not by a handle. `href -> Url` is a pure
  function, so a hit cannot give a different answer from a miss — which means
  nothing on the JS side changes, no object owns a host-side resource, and there
  is nothing to free. A handle scheme would have bought the same speed while
  making every `new URL()` allocate host state reclaimed only when a
  `FinalizationRegistry` callback happened to run.

- **An async op that was already finished still waited for a loop turn.** Many
  ops are async only in shape: the filesystem's `exists` answers from one
  `try_exists` and hands back `std::future::ready(..)`. The dispatcher registered
  it as pending work regardless, so its promise could not settle until the driver
  came back round — one full round trip per call, for a syscall that had already
  returned. The dispatcher now polls once before registering and settles the
  promise there if the future is ready.

  This does not make any op synchronous: the promise is *resolved*, not its
  reactions run — those still wait for the microtask checkpoint, so `await`
  suspends and resumes exactly as before. 6–10% off every small filesystem
  operation, `fetch` and `timers`.

- **`atob` copied its input a byte at a time.** The whitespace strip the spec
  calls for pushed every byte through a `Vec`, which cost more than the base64
  decode it was preparing. It now scans first and borrows the input untouched
  when there is no whitespace to strip, which is nearly always. `base64` 29.5ms
  → 22.2ms.

### Added

- **Every benchmark row now reports its memory, not just its time.** Peak
  resident set is sampled for all 61 published rows rather than three, so each
  cell reads as "234ms / 32MB" — how much RAM a runtime needed to do the work,
  which for a server is often the half of the question that decides. It was
  narrowed to three rows once because the numbers went unread; they went unread
  because they were never published, and a matrix of nulls is not evidence that
  nobody wanted them. `RSS_ROWS` narrows it again for a fast iteration loop.

- **The benchmark data describes itself, and the site renders from it.**
  `bench/run.sh` now carries one table of row definitions — group, unit, where
  the row is shown, and what to call it — and publishes it as `rows` and
  `groups` alongside the numbers. The benchmarks page asks for a group and gets
  whatever that group holds; the home-page roller asks for the rows marked
  `card`; `metric-direction.js` reads each row's better-direction from the same
  place. That retires 61 hand-kept `{ key, label, unit }` literals in the MDX, 39
  more in the roller, and a third list of higher-is-better keys, each of which
  had to be edited by hand whenever a row was added and none of which knew when
  it had gone stale.

  `bench/validate-bench-data.mjs` now checks the two agree in both directions: a
  group the page names but the run does not define would render an empty
  section, and a row measured but reaching no chart is a result quietly dropped.
  The home page is a shop window and shows a subset on purpose; the benchmarks
  page must show everything, and that is now enforced rather than remembered.

- **Sustained throughput, published next to the burst.** `rps.sh` could already
  hold load for a wall-clock window instead of firing a fixed burst, but nothing
  ran it and nothing charted it. `SECTIONS=rps_sustained` runs the same Hono
  server for 60s and publishes it as `hono_sustained`, and the benchmarks page
  puts the two side by side with the change between them. The burst answers how
  fast a runtime is when fresh; this answers whether it is still that fast once
  the heap has filled and the collector has been running throughout — which is
  the question a long-lived server actually poses.

- **The Hono req/s chart is on the benchmarks page too.** It was on the landing
  page only, which made the marketing surface the sole home of a measured
  result.

- **Scoped *publishing*, not just scoped runs.** The data module is fed by five
  independent scripts and re-running all of them takes most of an hour, which
  made changing one number an all-or-nothing event. `SECTIONS` picks which
  actually run — `workloads`, `rps`, `rps_static`, `websocket`, `http2`,
  `memory_safety` — and anything left out keeps the values already published.
  `workloads` still replaces the row matrices outright rather than merging, so a
  row deleted from the suite cannot live on in the data forever — unless it is
  scoped to named rows, which merges instead.

  Row arguments and sections are now the same mechanism rather than two
  incompatible modes. They could not previously be combined, which made adding a
  row and a section in one pass impossible: each failed validation waiting on the
  other, and the only way through was a full-suite run to change two numbers.

- **Static-file serving, measured externally.** `scripts/staticserver.js` serves
  a 64 KiB file per response and is driven by `rps.sh`'s external load generator
  on separate cores, so the number is the server alone. This is where the
  runtimes differ structurally rather than incrementally: Bun and Deno hand a
  file handle to the kernel and never touch the bytes, Node streams it, esrun
  reads it into a buffer. The `fsread` rows measure reading a file; this measures
  reading it *and* getting it onto a socket.

- **A `spawn` workload, and a `system` group to hold it.** 200 × (start
  `/bin/echo`, drain its stdout, wait for exit) through each runtime's own
  surface — `Deno.Command`, `Bun.spawn`, `node:child_process`, and esrun's
  `runtime:system` `Command`. esrun is the fastest measured at 84ms against
  Node's 211ms.

  The Node branch uses `spawn` rather than `execFile` deliberately: LLRT ships
  the former and not the latter, and reaching for the convenience wrapper
  recorded LLRT as unable to start a process at all — the same mistake the fs
  rows once made with `unlink`, caught this time before it was published.

- **`headers` and `formdata` workloads.** Header handling runs on every request a
  server answers, and a case-insensitive multi-map with ordering rules is more
  work than it looks; multipart parsing is what a file upload costs, on
  untrusted input. Neither was measured.

- **The memory-safety probe now runs, and reaches the site.** It asks three
  scripts for more memory than the machine can give and records only how the
  runtime refuses — a catchable error is a failed request, a signal is a dropped
  process. It had been invoking `esrun <path-to-esrun>` and looking for LLRT at
  `../llrt`, so neither ever ran, `deno` was called without `run -A`, and the
  results reached the site not at all. It now shares run.sh's runtime detection
  and publishes a `memory_safety` section. First finding: LLRT takes a SIGSEGV
  on a deeply nested `JSON.stringify` where every other runtime fails cleanly.

- **Scoped benchmark runs.** Rows are now grouped — `launch`, `engine`,
  `webapi`, `crypto`, `fs`, `net`, `serialization`, `wasm`, `wasi`, `memory` —
  and a run can take any subset: `GROUP=fs bench/run.sh`, `GROUP="engine
  crypto" bench/run.sh`, or `WORKLOADS="regex strings" bench/run.sh` to name rows
  directly. The full suite is long enough that working on one area meant either
  waiting for all of it or hand-assembling a `WORKLOADS` list from memory.

  `bench/run.sh --list` prints the groups and, more usefully, any script in
  `scripts/` that no group claims. Two workloads were sitting there unrun before
  this existed; now adding a file without wiring it up is visible in one command.

- **Ten benchmark workloads, covering what a server actually spends time on.**
  The suite measured JSON, URLs and encoding thoroughly while never once touching
  a regular expression. Added: `regex` (route match, field validation, global
  replace), `strings` (interpolation, rope-building concatenation, header
  splitting, search and slice), `errors` (throw/catch across frames *including*
  the `.stack` capture, which is the expensive half), `buffers` (TypedArray and
  `DataView` work, the layer every binary protocol sits on), `date_intl` (the
  ICU-backed formatters), `crypto_asym` (ECDSA P-256 sign + verify) and
  `crypto_kdf` (PBKDF2), since the crypto rows covered only symmetric work.

  `modules` loads a generated 300-module graph: `bigscript` measures parse
  throughput on one large file, but a real cold start is mostly resolution,
  instantiation and linking across many small ones.

  `rss_load` holds a 200 000-entry working set while churning garbage against it,
  and publishes `rss_loaded` beside the existing `rss`. The old row measured peak
  memory on a near-empty process: the floor, which is the figure every runtime
  looks best on and the least like production.

- **The machine is recorded next to the numbers.** `BENCH_JSON` now emits an
  `environment` block — OS, arch, CPU model, core count, filesystem type and
  frequency governor. The filesystem rows are meaningless without it (an append
  benchmark on ext4, on tmpfs and on a Docker overlay are three different
  measurements) and the launch rows move with the governor.

- **A handshake timeout and a connection cap for the WebSocket server**
  (DECISIONS D47). It was the least defended of the three servers: `runtime:http`
  had connection timeouts and a connection cap, and the WebSocket server had
  neither. A peer could complete the TCP handshake, never send its upgrade
  request, and hold a task and a file descriptor indefinitely — tungstenite waits
  for that request forever — and nothing bounded how many connections one server
  accumulated.

  ```js
  serve({ port: 4001, timeouts: { handshake: 5_000 }, maxConnections: 10_000 });
  ```

  `timeouts.handshake` defaults to **10s** and bounds only the opening handshake
  — RFC 6455's is an HTTP request head, so this is the same slowloris bound the
  HTTP server puts on the same bytes. An **established** connection is never
  touched: a socket silent for a week is idle, not stalled, and closing it is the
  application's decision.

  `maxConnections` is unlimited by default and holds rather than refuses, exactly
  as the HTTP server's does. It is worth setting here more than there: HTTP
  connection counts are self-limiting, while WebSocket connections are long-lived
  by design, so this decides whether the count has an upper bound at all.

  Both options are spelled as `runtime:http` spells them. **Embedders:** the
  provider trait's `serve(host, port)` is now `serve(WsServeOptions)`.

- **`close()` on a WebSocket server actually stops it.** The accept loop was the
  only one of the three servers with no handle kept for it, relying instead on
  its channel closing — which it can only notice *after* an accept returns. With
  a cap in force it parks on a connection permit first, where no arriving
  connection can wake it, so the listening port stayed bound until a slot freed;
  for long-lived WebSocket connections, indefinitely. And an `accept()` already
  parked when `close()` ran held the receiver checked out, so nothing could
  resolve it and `for await (const ws of server)` never ended.

  The acceptor is now aborted on close, as `runtime:net`'s listener already was:
  the port comes back immediately and a parked `accept` resolves to `null`.
  Connections already accepted are untouched.

- **The servers report their connection failures.** A failed TLS handshake, a
  WebSocket handshake a client got wrong, and a connection hyper ended on a
  protocol error were all dropped silently by `runtime:http`, `runtime:net` and
  the WebSocket server. Ending the connection quietly is right — a peer must
  never be able to take an acceptor down — but it left an operator with nothing,
  and a TLS listener whose chain no client will accept is otherwise
  indistinguishable from a listener nobody is calling: both serve zero requests
  and say nothing.

  All of them now log at `debug` on the `runtime::http`, `runtime::net` and
  `runtime::websocket` targets, with the peer and the reason:

  ```bash
  RUST_LOG=runtime::http=debug esrun server.js
  ```

  `debug`, never `warn`, because these are peer-driven: warning per connection
  would hand any client a lever on your log volume, and a scanner sweeping a
  public port could write the disk full. Accept-loop errors stay at `warn` —
  those are about the listening socket, which is the operator's problem.

  Each served connection is a `debug` span carrying its peer, so the events are
  attributable and correlated across the connection's life — per connection
  rather than per request, since on HTTP/2 one connection carries hundreds of
  requests and the peer is all they share. A connection that is served and
  closed cleanly logs nothing, and neither does one reaped by the idle
  keep-alive deadline: that is a healthy connection's designed end, not a fault.

- **A connection cap for `runtime:http`** (DECISIONS D45). `serve({ maxConnections })`
  bounds how many connections one server holds at once. Unlimited by default —
  the right number follows from your file-descriptor budget and the memory a
  connection costs, neither of which the runtime can read, and Node, Deno and Go
  leave it unlimited too. Worth setting on a public port: an HTTP/1.1
  connection's read buffer can reach ~408KB, so the connection count multiplies
  straight into memory.

  A connection over the cap is **held, not refused**. The limit is enforced by
  not accepting, so it waits in the kernel's backlog and is served as soon as a
  slot frees; nothing is spent on it meanwhile — no descriptor, no task, no read
  buffer.

  The per-connection limits it multiplies against are now stated in our code at
  hyper's own current values rather than inherited: 100 header fields and a
  ~408KB read buffer on HTTP/1.1, a 16KB header list on HTTP/2. No behaviour
  change — but a hyper release adjusting a default can no longer quietly adjust
  ours, which is how the 30s header timeout went missing.

- **The client's address, in `runtime:http`** (DECISIONS D44). The handler takes an
  optional second argument describing the connection:

  ```js
  serve({ port: 8080 }, (request, info) => {
    info.remoteAddr; // { transport: "tcp", hostname: "203.0.113.7", port: 54321 }
  });
  ```

  The same shape `Deno.serve` passes, so a handler ports either way, and the
  Fetch `Request` is left alone. A one-parameter handler is unaffected.

  `remoteAddr` is the **socket** peer and only that — behind a reverse proxy it
  is the proxy. `X-Forwarded-For` is deliberately never consulted: resolving it
  takes knowing which hop to trust, and a header anyone can send is not an
  identity. The header is delivered untouched for deployments that do know. It
  is `null` when the host has no peer to report, rather than a blank address.

  Costs 1.4% of throughput on the connection-per-request shape (67,697 → 66,759
  req/s, interleaved best of 5) for the two values now crossing per request.

- **Connection timeouts in `runtime:http`** (DECISIONS D43). A connection that is not
  making progress is now closed. Previously none of them ever were: a peer could
  complete the TCP handshake and then say nothing — one syscall, no state to keep
  — and hold a task and a file descriptor for as long as it liked.

  Three stages, each settable per server through `serve({ timeouts })` and
  disabled with `null`:

  | Option | Default | What it bounds |
  | --- | --- | --- |
  | `handshake` | `10000`ms | Accept → ready to carry requests: the TLS handshake, and the wait for the first byte the HTTP version is read from |
  | `headerRead` | `30000`ms | A request head arriving in full; on HTTP/1.1, the idle keep-alive limit too |
  | `h2KeepAlive` | `20000`ms | PING probes on an idle HTTP/2 connection — a dead peer is reclaimed within twice this, rather than waiting on the OS TCP keepalive (two hours by default on Linux) |

  They bound only connections that are **idle or stalled**. A request in flight,
  a body still arriving, and a response still streaming are never interrupted,
  however long they take. They are on by default because a timeout nobody
  configures protects nobody.

  Two things to know before upgrading. An **idle HTTP/1.1 keep-alive connection
  is now closed after 30s** where it previously lived forever — clients reopen
  transparently and this matches nginx (75s) and Node (5s), but it is a
  behaviour change, not only a hardening. And embedders: `HttpServeOptions`
  gains a `timeouts` field, so a struct literal needs updating.

  Not covered, and still unbounded: a slow request *body*, total request
  duration, and the number of concurrent connections.

### Fixed

- **The benchmark published four claims about other runtimes that were not
  true.** A fairness audit of `bench/` found the harness sound — interleaved,
  shuffled, min-of-N, with an equal sample count per row — but four defects in
  the workload scripts, all of them tilted the same way.

  Every filesystem workload tore its scratch file down with `fsp.unlink()`,
  placed *after* the timing but *before* the `RESULT_MS` print. LLRT ships `rm`
  but no `unlink`, so the call threw, the measurement was discarded, and the site
  recorded `unsupported` — for eight rows LLRT runs perfectly well, three of
  which it now **wins**. Cleanup uses `rm(…, { force: true })` and runs after the
  result is reported: a teardown failure can no longer erase a good measurement,
  which is the difference between "this run could not measure it" and "this
  runtime cannot do this". (`fsappend_*` stays n/a for LLRT — that gap is real.)

  YAML and TOML were benchmarked against `js-yaml` and `@iarna/toml` on *every*
  non-esrun runtime, and the page said those runtimes "lack native extensions".
  Bun ships native `Bun.YAML` and `Bun.TOML`, roughly 2x faster than the
  libraries it was being held to; on `toml_large` that is the difference between
  esrun placing first and Bun beating it by ~3x. Each runtime now parses with the
  best facility it actually ships, and the page says so.

  The serialization workloads warmed up for a flat 5 iterations before 500–1000
  timed ones, where the engine workloads use a tenth of the timed run. Measured
  cost: ~10% for the JIT-backed libraries, zero for native parsers — a systematic
  tilt applied to exactly the rows esrun wins widest. Warmup is now `max(N/10, 5)`
  throughout.

  `fsappend_large` appended 2 MB x 20, growing the file to 42 MB per launch and
  writing ~1 GB across a full row. Past the kernel's dirty-page threshold that
  measured the writeback scheduler rather than the runtime: run-to-run variance
  hit **168%**, and it was charted anyway. It is now 256 KB x 60, sized between
  the writeback threshold above and the measurement floor below.

- **Two benchmark rows were measuring the same thing.** `fsstat_large` differed
  from `fsstat_small` only by stat'ing a 2 MB file instead of a 4 KB one — but
  `stat` reads an inode and moves no bytes, so file size is not a variable it
  has. The two rows agreed to within 4%, and `fsexists_small`/`fsexists_large` to
  within 0.2% (node 70.6 vs 70.5, bun 51.2 vs 51.0, deno 90.6 vs 90.6): four
  charted rows carrying two rows of information.

  Replaced by `fsstat_many` and `fsexists_many`, which probe 1 000 distinct paths
  rather than one path repeatedly. Path *count* is a real second dimension — it
  walks a directory's worth of dentries instead of hitting one cached entry, the
  way a static-file server or a module resolver does.

- **Noisy cells are marked on the charts, not just in the terminal.** The run
  computes a coefficient of variation for every cell and `results_cov` was
  published alongside the numbers, but the site rendered a wobbly figure
  identically to a firm one — 17 cells above 10% variation charted as though
  they were precise. Cells now carry a `~` with the actual figure on hover, and
  a chart containing one gets a one-line footnote.

- **`fsexists_*` was measuring what `fsstat_*` measures.** The existence check
  was `stat().then(true).catch(false)` on Node, Bun and Deno — the same syscall
  plus a promise — so two pairs of charted rows carried one pair's worth of
  information. Each runtime now uses its idiomatic API: `access()` on Node and
  LLRT, `Bun.file().exists()` on Bun, `runtime:fs` `exists()` on esrun, and
  `stat` on Deno, which ships no existence primitive. This was hiding a real
  result — Bun's native check is ~7x faster than its stat path (7.2ms vs 50.8ms).

- **The `http` row said more than it measured.** It drives each runtime's server
  from a client in the same process, so it measures the server and that
  runtime's `fetch` together; `bench/rps.sh` exists precisely because that is
  not server throughput. The harness had said so in a comment since the load
  generator was split out — the site charted it bare. Now stated on the page.

- **The benchmark gate now checks the number it actually publishes.**
  `bench/run.sh` reports each cell's `results_floor_gap` — how far the
  second-lowest sample sits above the lowest — and
  `bench/validate-bench-data.mjs` rejects a run where any cell exceeds 25%.

  Gating on coefficient of variation, the obvious choice, asks the wrong
  question: the published number is a *minimum*, chosen precisely because
  interference only ever adds time, so a single writeback stall sends CoV past
  100% while leaving that minimum untouched. A CoV gate duly rejected a run whose
  `fsappend_large` floors were corroborated to within 2-7% for node, deno and
  esrun. What it should catch is the opposite shape — a lone low sample nothing
  else comes near — which CoV scores no differently. Bun produced exactly that on
  the same row: a floor 668% below its own next-lowest reading.

- **`esrun` installed no `tracing` subscriber, so every event was discarded.**
  `init_tracing` existed, was documented as the helper the CLI and tests share,
  and had its own idempotence test — but nothing in the tree ever called it.
  Installing a subscriber is process-global, so a library crate must not do it;
  the binary is the only place it can happen, and the binary never did. This was
  invisible from either side: the emitting code was correct and its unit tests
  passed, because a test installs its own subscriber.

  `RUST_LOG` now drives the filter, defaulting to `warn` when unset — quiet in
  normal operation, since the only `warn!` sites are the three accept loops
  reporting a listening socket they could not accept on. `RUST_LOG=off` silences
  even those.

  ANSI colour is now gated on stderr being a terminal, and on `NO_COLOR` — the
  same test the CLI already applied to its own diagnostics. Piped to a file,
  escape sequences split `peer=1.2.3.4` across the escape that coloured the
  field name, so the line was not greppable.

- **An abandoned provider call no longer kills the resource it was waiting on.**
  Five calls — `runtime:http`'s `next_requests`, `runtime:net`'s `read` and
  `accept`, and the WebSocket provider's `recv` and `accept` — take a channel
  receiver out of a shared registry for the duration of their await, because the
  registry's lock cannot be held across one. The put-back only happened on the
  paths that reached it, so a caller who abandoned the future took the receiver
  with it: the resource stayed in the registry looking alive while reporting
  "closed" or "end of stream" to every later call. Silent and permanent.

  Guest JavaScript could not trigger this — the engine polls every pending op
  future to completion — but the provider traits are a public integration seam,
  so an embedder wrapping one of these in `tokio::time::timeout`, or racing it in
  a `select!`, could. The put-back is now a destructor rather than a code path,
  so it happens on completion, on cancellation, and on a panic in between.

- **A failed `accept()` no longer kills a server.** The accept loops behind
  `runtime:http`'s `serve()`, `runtime:net`'s `listen()` and the WebSocket
  server left their loop on
  the first error from `accept`, which ended the server permanently while the
  port stayed bound — nothing served, and nothing else able to take the address.
  The errors that trigger it are ordinary on a busy public port and say nothing
  about the listening socket: `ECONNABORTED` from a client that hangs up between
  the SYN and the accept, `EMFILE`/`ENFILE` from a momentarily full descriptor
  table, `EINTR` from a signal. All three loops now retry every error and end
  only when the server is closed. (The WebSocket one was found later, while
  writing its internals page — the same defect in a third place.)

  The retry waits, doubling from 5ms to a ceiling of 1s and resetting on the
  next accepted connection, so a persistent failure (a descriptor limit that
  stays hit) costs one wakeup a second instead of spinning a core. Each retry is
  logged at `warn` with the error and the wait (`runtime::http` / `runtime::net`
  targets), because a fix that turned a silent death into a silent stall would
  barely be a fix.

### Added

- **HTTP/2 in `runtime:http`** (DECISIONS D42). `serve()` now answers HTTP/2 as well as
  HTTP/1.1, on the same port, with the handler unchanged — one `Request` in, one
  `Response` out, whichever version carried it. The version is negotiated per
  connection: over TLS by **ALPN** (`serve` advertises `["h2", "http/1.1"]` by
  default, h2 first), and on a **cleartext** port by the HTTP/2 connection
  preface, which is **h2c by prior knowledge** — what a reverse proxy or a gRPC
  client terminating TLS in front of the runtime speaks. The deprecated
  `Upgrade:`-header dance is deliberately not implemented.

  Requests **multiplex**: many can be in flight on one connection and are
  answered in any order, which the request handoff already supported (responses
  are matched per request, not per connection). One TLS handshake serves a whole
  session, and headers travel HPACK-compressed. Concurrent streams per connection
  are capped, so one peer cannot open unbounded streams against the
  single-threaded isolate. `request.url` is rebuilt from HTTP/2's `:authority`
  where HTTP/1.1 uses `Host`, so the URL a handler sees is the same shape on
  both.

  Guests that need one version only still say so with `alpn`
  (`alpn: ["http/1.1"]` pins a TLS listener to HTTP/1.1). No new dependency: the
  `h2` implementation was already in the tree behind the HTTP/2-capable `fetch`
  client.

  Measured rather than assumed, with `bench/http2.sh` (new): on **one**
  connection carrying 50 concurrent streams — the shape a reverse proxy or gRPC
  client is in — throughput goes from 20,157 to 73,541 req/s (**3.65×**), which
  is also the fastest of the four runtimes measured on that shape (Bun 49,142,
  Node 39,700, Deno 39,209). Across **50** connections, where there is nothing to
  multiplex, HTTP/2 is overhead and *loses* (0.79×) — as it does for every
  runtime measured.

- **`bench/http2.sh`**: HTTP/1.1 vs HTTP/2 throughput per runtime, in two client
  shapes (wide: many connections; narrow: one connection, many streams), driven
  by an external load generator against each runtime's own server. Sampling
  follows `run.sh`'s methodology — interleaved and shuffled repetitions, a
  discarded warmup, best of N. Rows whose two versions come from two different
  servers (Node and Bun, whose cleartext h2 is `node:http2` rather than their
  default server) are marked, because their h2-vs-h1 ratio also carries an
  implementation change and is not comparable with the unmarked rows. A cell
  reads n/a when a runtime has no cleartext h2 server or no repetition was ≥99%
  successful.

- **Complete ESM package resolution: conditions, `imports`, and self-reference**
  (DECISIONS D40, lifting D22's deferral). Bare-specifier resolution now
  implements the whole `exports`/`imports` algorithm rather than its common
  subset:

  - **Conditions are matched in the order the package author wrote them**, and
    nested condition objects are walked, falling through to the next key when a
    matched branch resolves to nothing. The conditions asserted are
    **standards-only** — `import` and `default`. No runtime-branded key (`node`,
    `deno`, `bun`, or one of our own) is asserted, so a manifest's ESM answer is
    reached through `default` rather than through a private name.
  - **`imports` / `#private` specifiers.** `import config from "#config"`
    resolves against the nearest `package.json`'s `imports` map, which may point
    at a path in that package (including subpath patterns) or at another package.
  - **Self-reference.** A package that declares `exports` can import itself by
    its own `name`, so an intra-package import resolves to exactly what a
    consumer would get.
  - **Array fallbacks** (`["./a.mjs", "./b.mjs"]`) and **`null` targets**, which
    withdraw a subpath. A withdrawn subpath now reports that the author withdrew
    it instead of a generic "not found".
  - **Target validation.** A target (after any `*` substitution) may not contain
    a `..`, `.` or `node_modules` segment, may not be a bare specifier in
    `exports`, and may not be a trailing-slash directory mapping — so a pattern
    capture cannot walk out of the package it belongs to. The D25 project-root
    jail is unchanged and still applies underneath.

  A malformed manifest (an invalid target, or an `exports` object mixing subpath
  keys with condition keys) is now an error naming the `package.json`, not a
  silent "not found".

- **`import.meta.resolve` resolves bare and `#private` specifiers** (DECISIONS
  D41). It previously threw for anything the module loader had to answer, which
  left no way to locate a non-JS file shipped inside a dependency — a migration,
  a `.proto`, a template, a CA bundle — since the install layout (pnpm's store,
  hoisting) is exactly what resolution knows and a hardcoded path does not.

  ```js
  const schema = import.meta.resolve("my-orm/migrations/001.sql");
  ```

  Resolution is now a single synchronous core that the asynchronous
  `ModuleLoader::resolve` also calls, so `import.meta.resolve` and `import()`
  cannot disagree, and both pass the same root jail and import policy. It is
  gated on the `imports` permission like the loader itself: a denied run gets a
  refusal, not the location of a package. `ModuleLoader` gains an optional
  `resolve_sync` (defaulting to `None`) — a loader whose modules come from the
  network keeps the previous `TypeError`.

### Changed

- **An HTTPS `serve()` now advertises `h2` in ALPN by default** — `["h2",
  "http/1.1"]`, where it was `["http/1.1"]`. The visible effect on an existing
  deployment: an h2-capable client that was served HTTP/1.1 yesterday gets
  HTTP/2 today, from the same handler and the same code. Nothing in guest code
  can observe the version, so this is a wire change rather than an API one, but
  it is a change to what your clients negotiate — pin it back with
  `alpn: ["http/1.1"]` if a client mishandles h2. Cleartext listeners are
  affected too, by the HTTP/2 preface rather than by ALPN. See **HTTP/2 in
  `runtime:http`** above.

- `import.meta.resolve` names the kind of specifier it cannot resolve. A
  `#private` specifier was reported as a *bare* specifier needing `node_modules`;
  it needs the referring package's `imports` map instead, and now says so. Both
  still throw a `TypeError` — `resolve` is synchronous and either answer is host
  I/O.

## [0.14.0] - 2026-08-02

### Added

- **`esrun` permission flags** (DECISIONS D38). `esrun` still grants everything
  by default; restriction is opt-in and expressed as denials.

  ```sh
  esrun --deny-net --deny-run app.js                     # everything, minus these
  esrun --deny-all --allow-imports --allow-net app.js    # nothing, plus these
  ```

  Two modes, each with a single direction, and they cannot be combined:
  `--deny-<name>` subtracts from everything; `--deny-all --allow-<name>` adds to
  nothing. `--allow-<name>` requires `--deny-all` (with everything granted there
  is nothing for it to add), so **no flag ever overrides another** and there is
  no precedence to reason about. The names map 1:1 onto capabilities: `read`,
  `write`, `imports`, `net`, `listen`, `env`, `run`, `signals`. A denied
  operation throws `NotAllowedError` / `ERR_CAPABILITY_DENIED` before the effect.

  `--deny-all` still runs the entry file (read before the runtime exists), and
  since it includes `--deny-imports`, it alone is a single-file run — add
  `--allow-imports` for an app with dependencies.
  `Clock`/`Entropy`/`Timers`/`TaskSpawn` have no flag and survive it — no op
  gates them, so a denied script still computes.

- **Scoped grants** — seven of the eight `--allow-<name>` flags take a
  comma-separated list that narrows the grant instead of handing over the whole
  capability (DECISIONS D38). `imports` is the exception: what may be *loaded*
  is a separate mechanism (see `--import-policy` below).

  ```sh
  esrun --deny-all --allow-imports --allow-env=PORT,DATABASE_URL \
        --allow-net=db.internal:5432 --allow-listen=8080 \
        --allow-read=./data --allow-write=./out --allow-run=git \
        --allow-signals=SIGTERM server.js
  ```

  `--allow-signals` also hides unlisted signals from `signals()`.

  Paths are resolved against the working directory, cover their subtree, and are
  checked **after canonicalization**, so a symlink cannot walk out of a list. A
  path list narrows the root jail and never widens it.

  `--allow-net` is what stops an exfiltration: a compromised dependency reaching
  out over the app's own legitimate network access now has to reach an address
  you named. It is enforced **on every redirect hop**, not just the URL the
  program wrote — a `302` from an allowed host to a denied one fails rather than
  being followed. `net` and `listen` keep separate lists: reaching out and being
  reachable are separate capabilities.

  An address is a host (any port), a `host:port`, or a bare port (any
  interface); `[::1]:8080` for IPv6. Matching is exact — `example.com` does not
  admit `api.example.com`, and there are no wildcards — and hosts are judged as
  written, before resolution.

  Unlisted environment variables are **absent** from `env`, so the guest cannot
  read them or even enumerate their names; an unlisted program fails to spawn
  with `ERR_PERMISSION_DENIED` — a *scoped* denial, distinct from the
  `ERR_CAPABILITY_DENIED` that `--deny-run` produces.

  Entries are comma-separated and trimmed (`--allow-env="A, B"` ≡
  `--allow-env=A,B`); an empty entry (`a,,b`) is an error. Repeating a flag
  unions its entries; granting one capability both whole and narrowed
  (`--allow-env --allow-env=HOME`) is an error rather than a precedence rule.
  A scoped grant still reports `permissions.has("env") === true` — the
  capability is granted, the provider is what narrows it.

  **`--allow-imports` takes no value** (`--allow-imports=./lib` is **rejected
  rather than ignored** — a run must never look narrower on the command line
  than it is in reality); use `--import-policy` instead. A denial takes no value
  at all either: a scope narrows a grant.

- **`--import-policy=<file>`** — what a run may *load*, as a JSON file rather
  than a capability scope (DECISIONS D39). Capabilities bound what running code
  may reach; which modules may *become* running code is a different question,
  with different needs (aliases, warn-only rollout, integrity), none of which
  belong on a permission flag.

  ```sh
  esrun --deny-all --allow-imports --import-policy=./import-policy.json server.js
  ```

  ```json
  { "allow": ["./src", "express", "@acme/ui"], "deny": ["aws-sdk"] }
  ```

  An entry beginning with `.` or `/` is a path covering its subtree; anything
  else is a package name — the split the loader already makes between a relative
  and a bare specifier. **Deny wins over allow.** Omitting `"allow"` permits
  everything not denied; an empty `"allow": []` and any unknown key are errors.
  Paths resolve relative to the **policy file**. Matching runs on the resolved,
  canonicalized module (after the root jail), so a symlink cannot name its way
  in and a pnpm store path is still recognisably its package; a package entry
  covers that package's own files and not the packages *it* imports. The entry
  file is exempt, and the file is never auto-discovered.

  The policy is a second layer, not an alternative: the `imports` capability
  decides whether the loader runs at all, so a policy is **not** a way around
  `--deny-imports`. It names packages, not content — integrity pinning is future
  work.

- **`permissions` on `runtime:process`** — what this process is allowed to
  reach. The policy is fixed at launch, so it is introspection only: a
  synchronous `has()`, no `request()`, no prompt.

  ```js
  import { permissions } from "runtime:process";
  permissions.denied;      // ["read", "write"]
  permissions.has("net");  // false
  ```

  Needs no capability, so it answers even under `--deny-all`. It takes **one**
  argument: `has("read", "/etc/passwd")` throws rather than answering about the
  capability and ignoring the path. Scoping is set by the deployment; the exact
  answer for one value is to perform the operation and catch
  `ERR_PERMISSION_DENIED`.

### Changed — **breaking**: one argument grammar for the whole CLI

Every `esrun` flag is now `--flag` or `--flag=value`. **A value is never a
separate argument**, and esrun's flags must come **before** the script.

```sh
esrun --timeout=500 app.js          # was: esrun --timeout 500 app.js
esrun --env-file=.env app.js        # was: esrun --env-file .env app.js
esrun -e='console.log(1)'           # was: esrun -e "console.log(1)"
esrun --shutdown-grace=30000 app.js # was: esrun --shutdown-grace 30000 app.js
```

The space form now fails with a message naming the fix. One rule replaces two:
previously the permission flags required `=` while the older flags took a
following word, and that inconsistency is what lets `--allow-net example.com
app.js` quietly run `example.com` as the script. With a single rule the parser
never has to decide whether the next word is a value or the script, so it cannot
decide wrong.

Rule 2 is enforced too: a flag esrun knows, written **after** the script, is an
error rather than a silent no-op — for `--deny-net` that silence would be a
security failure. `--` after the script opts a script's own argument out.

### Fixed

- **A handled socket failure no longer also fails the run.** One connect (or
  bind) failure rejects several promises — `opened`, the streams, `close()`, a
  later `startTls()`, and `Listener.addr` — because all of them derive from the
  same pending operation. A program can only handle one of those, so the
  leftovers reached the global scope as unhandled rejections and ended the
  process with a non-zero exit *even though the error had been caught*. For a
  server that meant one unreachable host could take down a process that had
  already dealt with it.

  `opened` and `addr` are now built as handled, which drops the duplicate report
  and not the error: `await sock.opened` still rejects, and the streams still
  deliver the failure to their reader. The tradeoff is deliberate — a socket
  nobody ever consumes now fails silently instead of ending the run.

  Not specific to the permission flags: an ordinary DNS failure behaved the same
  way. Four regression tests in `tests/errors.rs`, three of which fail without
  the fix and one of which guards against over-fixing it into a swallowed error.

- **`runtime:process` no longer needs a capability to import** (DECISIONS
  D26/D38). It called `Env`-gated ops at module-evaluation time, so denying
  `Env` made the module unimportable — taking `exit()` and `onSignal()`, which
  have nothing to do with the environment, down with it. `env` and `args` now
  seed on first access; a test asserts every `runtime:` module imports and
  evaluates with no capabilities granted.

### Changed

- **`process_exit`, `process_platform`, and `process_arch` are no longer gated
  on `Env`.** Stopping is the guest's own control flow (neither Node nor Deno
  gates it), and the platform strings are properties of the binary already
  running, not host state.
- A capability denial now names the permission alongside the capability —
  `capability denied: FileSystem (permission "imports")` — so the message points
  at the flag that produced it.

- **Website updates**:
  - Set Open Tech Foundation org logo as the site favicon.
  - Formatted Migration Guide table code cells to prevent import statement line breaks.
  - Upgraded site framework packages (`@opentf/web` to `v0.27.0`, `@opentf/web-docs` to `v0.25.0`, and `@opentf/web-cli` to `v1.25.0`).
  - Updated Built With footer badge styling to white background with black "OTF" brand text and orange "Web" accent.
  - Added missing `--color-brand-950` theme color in `global.css` to fix landing page Alpha badge dark mode.
  - Updated website config version to `v0.13.0`.
  - Enforced default dark background on site footer.
  - Converted CTA section to standard theme-responsive page section.
  - Added Subprocess (`ffmpeg`) example tab to code samples and removed glob scanning tab.
  - Replaced text ticks/crosses and emojis with vector SVG status components across landing page and documentation tables.
  - Expanded Migration guide with comprehensive side-by-side examples from Node.js, Bun, and Deno, API mapping cheat sheet, and pre-flight checklist.
  - Upgraded site framework packages `@opentf/web-docs` to `v0.24.0` and `@opentf/web-cli` to `v1.24.1`.
  - Removed the landing page "Why ESRun?" runtime comparison grid. The core
    feature cards are now the "Why ES-Runtime?" section and sit above the
    architecture diagram.

## [0.13.0] - 2026-07-31

### Added

- **`runtime:system` — child processes** (DECISIONS D37). The last "a server has
  to shell out" gap: transcode an upload with `ffmpeg`, read a repo with `git`,
  drive a sidecar over pipes, run a deploy step.

  ```js
  import { Command } from "runtime:system";

  const { stdout } = await new Command("git", { args: ["rev-parse", "HEAD"] }).output();

  const child = await new Command("ffmpeg", {
    args: ["-i", "pipe:0", "-f", "mp3", "pipe:1"],
    stdin: request.body,          // any web body pipes straight in
  }).spawn();
  return new Response(child.stdout);   // and straight back out
  ```

  `Command` carries `args`, `cwd`, `env`, `inheritEnv`, `stdin`/`stdout`/
  `stderr`, `signal`, `timeout`, `killSignal` and `maxBuffer`; `output()`
  collects, `spawn()` gives a `ChildProcess` with web-stream `stdin`/`stdout`/
  `stderr`, a `status` promise, `kill(signal?)`, and `Symbol.asyncDispose`.

  Four choices worth knowing about, each different from Node/Deno/Bun:
  - **A new `Capability::Run`, never implied by another.** A child runs outside
    every confinement here — no capability check, no root jail, no execution
    deadline — so granting it grants everything the host user can do. The
    default provider takes a policy for embedders that must grant it anyway:
    `SystemCommands::with_allowlist([...])`, `with_max_children(n)`.
  - **No shell.** No `exec`, no `shell: true`, no template form: a command is a
    program plus an argv, so a guest-supplied argument can never become a second
    command. Windows `.bat`/`.cmd` are refused rather than run through
    `cmd.exe` (CVE-2024-27980).
  - **No inherited environment.** A child gets exactly the `env` passed;
    `inheritEnv: true` additionally requires `Env`. This closes the D26/D30
    deferral on env propagation. A `Secret` is unwrapped for the child (it would
    otherwise arrive as the literal `"[redacted]"`) while still masking
    everywhere else.
  - **`output()` is bounded** by `maxBuffer` (16 MiB default, `ERR_MAX_BUFFER`
    past it), and children still running at shutdown are killed rather than
    orphaned.

  `Signal` gains send-only `SIGKILL`/`SIGQUIT` (never watchable) so a kill can
  escalate. Embedders wire it up with `HostProviders::with_commands`; without a
  provider the ops fail cleanly, like a denied capability.

### Fixed

- **`fetch` honours `redirect: "manual"` and `"error"`.** It never had: the mode
  was parsed, stored and reported on the `Request`, but nothing read it and it
  never reached the transport, so every redirect was followed no matter what the
  caller asked for. `redirect: "manual"` is the standard way to inspect a `3xx`
  without walking it — the guard code reaches for when a URL is attacker-
  influenced and a redirect could point somewhere internal — so a mode that
  silently does nothing is worse than one that is absent. All three modes now
  work, the mode travels to the transport (`HttpRequest.redirect`) rather than
  being interpreted after the fact, and `"follow"` is capped at the
  specification's 20 hops with `ERR_TOO_MANY_REDIRECTS` past it instead of
  reqwest's default 10.
  One deliberate deviation, recorded in `SPEC.md` §7: `"manual"` returns the real
  `3xx` rather than the spec's opaque-redirect filtered response (status `0`, no
  headers). That filtering protects a *browser's* cross-origin navigations; here
  it would only make the mode useless, since reading `Location` is the entire
  reason to ask for it. Node, Deno and Bun all return the real response too.
- **`Response.redirected` was hardcoded `false`**, and so was useless for the one
  thing it exists for: noticing that a request did not end up where it was sent.
  It now reports what the transport actually did, alongside a `response.url` that
  is the final URL. Neither can be forged — a script-constructed `Response`
  passing `{ redirected: true }` still reads `false`.
- **An unknown `redirect` value is now a `TypeError`.** `new Request(url, {
  redirect: "manaul" })` used to be accepted and stored verbatim; now that the
  value decides whether a `3xx` is followed, a typo silently meaning `"follow"`
  is a bug that only shows up in production.
- **A `fetch` could hang forever on a peer that never completed a handshake.**
  The client was built with `Client::builder().build()` and nothing else, so it
  carried no connect timeout at all: a host that accepted a TCP connection and
  then stalled in TLS, or a black-holed address, parked the request until the
  process died. DNS + TCP + TLS is now capped at 30 seconds, failing with the
  existing `ERR_TIMED_OUT`, and pooled connections carry a 60-second TCP
  keepalive so a peer that vanishes without a FIN is not handed to a later
  request.
  The request **as a whole** is deliberately left uncapped. Fetch defines no
  timeout, and a response body may be long-lived by design — server-sent events,
  a log tail, a large download — so a default deadline would break correct
  programs. That call belongs to the caller, and Fetch already hands them the
  tool: `fetch(url, { signal: AbortSignal.timeout(ms) })`, which works and is now
  documented as the answer.
- **A `runtime:http` handler now learns that the client hung up.** The
  `request.signal` handed to a handler was a fresh `AbortController`'s signal
  that nothing was ever wired to, so it could not fire: a handler had no way to
  know its caller had gone, and expensive work ran to completion writing to a
  socket nobody was listening on. It now aborts with an `AbortError` when the
  peer disconnects before the response was handed over, and composes with
  everything else that takes a signal — `fetch(url, { signal: request.signal })`
  drops the upstream call the moment the caller does.
  Reading `request.signal` is what starts the watch on the connection, so a
  handler that never asks costs nothing: the watch holds a pending op for the
  life of the request, and charging every request for a signal most handlers do
  not read would have halved the effective concurrency under the
  `max_pending_ops` bound. Same deal, and the same reasoning, as the deferred
  `request.headers`.
  The signal covers the window *before* the response is handed over; a client
  vanishing partway through a streamed response body is a different event, and
  was already reported by that stream ending.
- **`fetch` decodes compressed response bodies.** The default transport was
  built without any content-coding support, so it never sent `Accept-Encoding`
  and, worse, never decoded: a server that compressed anyway — plenty do it
  unconditionally — handed the guest raw gzip bytes, and `await r.text()`
  returned binary garbage with no error to say why. `Accept-Encoding: gzip, br,
  deflate` now goes out (the same set `CompressionStream` implements) and a
  response in any of those codings arrives decoded, with `Content-Encoding` and
  `Content-Length` dropped so they cannot describe bytes the guest never sees.
  Decoding keys off the response's `Content-Encoding` rather than off who asked,
  so an unbidden compressed response is handled too. A coding the client does
  not implement passes through untouched, headers intact.
  `zstd` is deliberately not included: nothing else in the runtime speaks it, and
  advertising a coding means carrying a codec that exists for no other reason.
  Only three crates are new — `brotli` and `flate2` were already in the tree for
  `CompressionStream`.
- **Outbound requests identify the runtime.** `fetch` sent no `User-Agent` at
  all, which some CDNs and WAFs treat as a bot. Requests now carry
  `ES-Runtime/<version>` — the same string as `navigator.userAgent`, so a server
  sees the identity the guest reports — unless the request sets its own. Like
  the content-codings above, this is a property of the default
  `ReqwestTransport`; an embedder with its own `NetTransport` decides for itself.
- **Embedders implementing `HttpServerProvider`:** a new `request_disconnected`
  method backs the handler's `request.signal`. It has a default returning
  `false`, so an existing transport keeps compiling and simply has a signal that
  never fires; implement it to opt in. It **must** settle either way — a future
  that never resolves would hold a driven loop open for the life of the process.
- **Breaking (embedders implementing `NetTransport`):** `HttpRequest` gains a
  `redirect: RedirectMode` field and `HttpResponse` a `redirected: bool`. A
  transport must decide between `RedirectMode::Follow` and `Manual` and report
  whether it followed anything. Fetch's third mode, `"error"`, deliberately does
  *not* appear at the seam — it is a rule about the resulting response rather
  than about the wire, so the runtime asks for `Manual` and rejects itself,
  instead of obliging every transport to reimplement the same check.
- **A malformed `URLPattern` no longer panics inside the host.** Found by the new
  fuzzing: `new URLPattern("**:]:")` — an unmatched `]` where the hostname would
  be — underflows `urlpattern` 0.6.0's IPv6 bracket-depth counter. The op
  boundary already contained it (D15), but as a generic "internal error in host
  op" rather than the `TypeError` the spec names for a pattern that cannot be
  parsed. It is now caught where it happens and reported as a `TypeError`, with
  the crashing input kept as a fuzz seed so it can never come back unnoticed.
- **Release builds check integer overflow.** A wrapped counter in a parser
  reading guest-chosen input is a security bug, not a performance trade — the
  `URLPattern` finding above is exactly that shape. `overflow-checks` is now on
  in the release profile, so an overflow is a contained exception rather than a
  parser silently continuing in a state that cannot occur. The cost does not
  show: the hot paths are V8's, and V8 is a prebuilt static library this never
  touches.

- **`esrun types` was missing three of the eight `runtime:` modules.** The
  bundle is a hand-written list, so `runtime:websocket`, `runtime:serialization`
  and `runtime:wasi` had never been added to it — anyone who ran
  `esrun types --install` got no types for them at all, with no error to say
  why. All eight now ship, and two tests walk `types/` rather than trusting the
  list: one asserts every `runtime-*.d.ts` is in the bundle, the other that it is
  referenced by `index.d.ts` (the npm package's entry point).
- **`runtime:websocket` had no TypeScript definitions at all**, though it has
  shipped since 0.11. Added, covering `serve`, `broadcast`, `WebSocketServer`
  and the connection surface.
- **Documentation caught up with what actually ships.** `API.md` and the site
  both listed `MessageChannel` under "not available" while it, `MessagePort` and
  `BroadcastChannel` had shipped. Undocumented in both: `URLPattern`,
  `URL.createObjectURL`/`revokeObjectURL`, User Timing, `ReadableStream.from`,
  byte/BYOB streams, `Headers.getSetCookie`, the `Response` statics, and
  `CompressionStream`/`DecompressionStream` on the site. In `SPEC.md`: §2.4 still
  described `URLPattern` as a hand-written JS implementation, §2.5 marked timers
  in progress, §3 listed the `FileSystem` provider as "later" (and omitted the
  five providers added since), and phase 11 still listed `runtime:path` and
  `runtime:fs` as remaining. All corrected.

### Added

- **`runtime:fs` gained the file operations it was missing** — `copy`,
  `realPath`, `readLink`, `truncate`, `chmod`, `makeTempDir` and `makeTempFile`.
  The module had ten exports and none of these, so copying a file meant reading
  it whole into memory and writing it back, resolving a symlink was impossible,
  and there was no way to create a scratch directory or to write a key file with
  `0600` on it.
  Temporary entries land in the **base directory**, not the OS temp directory:
  that is outside the root jail, so writing there would be the one filesystem
  call that escapes it. Names come from the host's temp-file machinery rather
  than being composed by the caller, because a guessable name in a shared
  directory is a symlink-attack invitation. Nothing is cleaned up automatically.
  `realPath` re-canonicalizes and re-checks the jail before answering — it is
  exactly the call that asks "where does this really point?", so answering with
  somewhere unreachable would defeat the point. `readLink` deliberately does not
  resolve the link it is asked about (which would read the target's target, or
  fail outright on a link to a regular file); its parent chain is still resolved
  and jailed.
  `chmod` applies a Unix mode as given. Windows has no such bits, so only the
  owner-write one is honoured, as the read-only flag — stated rather than
  silently pretended.

- **An op can now require more than one capability.** `copy` reads one path and
  writes another, and gating it on `FileWrite` alone would have let a guest with
  no read access duplicate a file it cannot see into somewhere it can reach by
  another route — an exfiltration primitive out of a write-only grant.
  `OpDecl::requires` is additive, so an op names every authority it actually
  exercises instead of whichever gate is convenient. Existing single-capability
  ops are unchanged.

- **`runtime:http` terminates TLS — `serve({ secureTransport: "on", cert, key })`.**
  It served plain HTTP only, so putting an ES-Runtime server on a public port
  meant a reverse proxy in front of it, no matter how small the deployment. The
  gap was also an odd one: `runtime:net` `listen` has terminated TLS since 0.11
  and `fetch` speaks TLS as a client, so the one thing that could not was the
  HTTP server.
  Options match `runtime:net` `listen` exactly, and the cert and key travel
  **inline** rather than as paths for the same reason: reading a file is the
  filesystem's privilege, so a guest serving HTTPS from a cert on disk reads it
  with `runtime:fs` under its own gate, and serving needs no grant beyond
  `NetListen`. Both providers now build their rustls config through one shared
  helper, so there is one thing to keep right rather than two.
  `request.url` reports the `https:` scheme, taken from the listener — a client
  cannot talk a plain server into claiming it with a `Host` header. An
  unparseable cert or key fails the `serve` call rather than each later
  handshake, and `secureTransport: "on"` without both is a `TypeError`: a bound
  port that rejects every connection looks like a working server nothing can
  reach. A failed handshake ends only its own connection — on a public port
  those are routine, and taking the acceptor down with one would be a
  single-packet denial of service.
  Still HTTP/1.1 only; `alpn` (default `["http/1.1"]`) is advertised for the
  client's benefit, not to switch protocol.
- **Breaking (embedders implementing `HttpServerProvider`):** `serve` now takes
  an `HttpServeOptions` struct instead of `(host, port)`, carrying an optional
  `HttpServerTls`. A struct so binding options can grow without breaking every
  implementation again.

- **`esrun` shuts down gracefully on `^C` / `SIGTERM`.** A running server was
  killed where it stood: in-flight requests died mid-response, and a client that
  had waited seconds for an answer got an empty reply. That is the default
  behaviour a container gets on every deploy.
  `esrun` now stops accepting, lets in-flight requests answer, and exits with the
  conventional `128 + signal` (`130` / `143`), which is what an orchestrator
  reads. `--shutdown-grace <ms>` bounds the wait (default `10000`).
  Three cases, deliberately distinguished:
  - **The guest installed a signal handler** — it owns shutdown, and `esrun`
    stays out of the way entirely rather than racing it.
  - **No server is running** — exit immediately. There is nothing in flight to
    protect, and making `^C` on a plain script wait out a grace period would be a
    regression, not a feature.
  - **Servers are running** — drain. A second interrupt during the drain exits
    at once: someone pressing `^C` twice means it.

  Draining waits for the **connections** to close, not merely for the handler to
  return. A response is *handed to* the HTTP transport and the guest moves on;
  the bytes reach the socket only when hyper is polled again, so exiting between
  those two points is precisely how an in-flight request becomes an empty reply —
  which is what the first draft of this did. Live connections are now put into
  hyper's own graceful shutdown, which finishes what is in flight and then
  closes, and the process waits for that.

- **OS signals — `runtime:process` `onSignal` / `offSignal` / `signals`.** There
  was no way to observe a signal at all, so a container's `SIGTERM` killed the
  process outright: in-flight HTTP requests died mid-response, connection pools
  and open files were never closed, and nothing a program did could change that.
  Graceful shutdown was simply not expressible.
  `SIGINT`, `SIGTERM`, `SIGHUP`, `SIGUSR1` and `SIGUSR2` are deliverable on Unix
  and `SIGINT`/`SIGBREAK` on Windows; `signals()` reports the set, and asking for
  one the platform lacks **throws** rather than registering a handler that could
  never fire.
  Gated on a new **`Signals` capability**, deliberately separate from `Env`:
  watching a signal suppresses its default action, so it is the privilege to
  decline to die on request, not a read of process state. Granting a program the
  environment must not also grant it the ability to ignore a shutdown.
  Delivery is pull-based, like the HTTP server's `next_requests` — the runtime
  owns no loop, so it asks for the next signal and awaits. That pending op is
  also what keeps the program alive to receive one (as in Node and Deno);
  removing the last handler releases it, so a program that stops listening can
  still exit. Repeated deliveries coalesce: a burst of `SIGHUP`s while the first
  is still being handled arrives once, because signals are edge notifications and
  replaying a backlog helps nobody. A handler that throws is reported like any
  other unhandled failure and does not stop the others.
  New `Signals` provider trait with `SystemSignals` (tokio) and `ManualSignals`
  (deterministic, for tests). A program that installs no handler is completely
  unaffected — `SIGTERM` still terminates it exactly as before.

- **Fuzzing and Miri in CI** — the two pre-1.0 gates SPEC §5 has been asking for.
  Six `cargo-fuzz` targets under `fuzz/` cover the parsers that read untrusted
  bytes: URL parsing and component read-back (where the UTF-16 offset arithmetic
  lives), `TextDecoder` across every label in both lossy and fatal modes,
  URLPattern constructor strings, decompression, XML, and the hand-written
  RFC 8410 key DER plus `atob`. CI runs each for 60 seconds seeded from
  `fuzz/seeds/`, which is committed and includes every input that has found a
  bug — the first run found one (see Fixed). Miri runs over `common` and
  `providers`, the crates that do not link V8.
  ASAN over the FFI surface is **not** included, and SPEC §5 now says why: the
  `v8` crate links a prebuilt static library, so a sanitizer would report that
  library's own allocations rather than any misuse of it. Doing it properly needs
  a source build of V8 with `-fsanitize=address`.

- **`import.meta.resolve`.** It was `undefined`, so the standard way to turn a
  path relative to a module into a URL — the usual reason to reach for
  `import.meta` at all — did not exist. It is pure URL resolution against the
  current module, with no I/O and no check that the target exists, which is
  exactly what Node's does. Relative, absolute-path and absolute-URL specifiers
  all resolve, including the `runtime:` scheme. A **bare specifier throws a
  `TypeError` naming it**: resolving one means reading `package.json` files and
  probing the filesystem, and `resolve` is synchronous — so it refuses rather
  than answer with a URL it never resolved. Recorded as a deviation in SPEC §7.

- **`console` implements the whole Console Standard.** Four methods were missing
  outright (`dirxml`, `clear`, `countReset`, `timeLog`) and four more were
  present but did nothing: `count`, `time` and `timeEnd` were empty functions,
  so code that instrumented itself with them silently measured and reported
  nothing, and `group`/`groupEnd` did not indent, so grouping structure was lost.
  Now: `count`/`countReset` keep per-label tallies, `time`/`timeLog`/`timeEnd`
  report elapsed milliseconds (and warn about a timer that does not exist or is
  started twice), `group`/`groupCollapsed`/`groupEnd` indent every following
  line — including the inner lines of a multi-line value — and `trace` prints an
  actual stack.
- **`console.table` renders a table** instead of dumping the object through
  `log`, which made the method pointless. Array indices or object keys become
  the first column, each row's own keys become columns, and rows that are
  primitives go under `Values`.
- **`console` format specifiers.** `%s`, `%d`/`%i`, `%f`, `%o`/`%O`, `%j`, `%%`
  and `%c` are applied per the standard's Formatter; leftover arguments are
  appended as before. `%c`'s CSS argument is consumed and discarded — there is
  no styling to apply to a provider sink, and printing the CSS would be worse.

- **`TextDecoder` now accepts every encoding the WHATWG Encoding Standard
  defines.** It was UTF-8 only: `new TextDecoder("utf-16le")` — or
  `windows-1252`, `latin1`, `shift_jis`, `gb18030`, or any of the other labels —
  threw `RangeError`, which is a hard stop for anything reading a file, a
  database column, or an HTTP response that is not UTF-8. Decoding and the label
  table both come from `encoding_rs`, the implementation Firefox ships, so
  `latin1` resolves to `windows-1252` and `utf-16` to `utf-16le` exactly as the
  standard says rather than by a hand-written subset. `fatal` and `ignoreBOM`
  work for every encoding, and a BOM is stripped only for the decoder's *own*
  encoding — a UTF-16 BOM handed to a `windows-1252` decoder is data, not a
  signal to switch.
  Streaming decode is now the host decoder's job rather than a UTF-8-shaped
  guess in JS: a character split across chunks survives the boundary for any
  encoding, including ISO-2022-JP's shift state, which no amount of
  byte-counting could have handled. A one-shot `decode(bytes)` still allocates
  nothing — only a `{ stream: true }` call takes a native context, released when
  the stream ends, with a `FinalizationRegistry` backstop for a decoder
  abandoned mid-stream.

- **Ed25519 and X25519 — the WebCrypto Secure Curves.** Both were
  `NotSupportedError`, while Node, Deno, Bun and Workers all ship them, so
  EdDSA-signed tokens and X25519 key agreement had no path through
  `crypto.subtle` at all. Ed25519 covers `generateKey`/`sign`/`verify` and
  X25519 `generateKey`/`deriveBits`/`deriveKey`, in every key format:
  `raw`/`spki` for public keys, `pkcs8` for private (RFC 8410 DER), and `jwk`
  as `kty: "OKP"`. Verified against the published vectors — RFC 8032 §7.1 for
  Ed25519, RFC 7748 §6.1 for X25519 — rather than only against themselves.
  X25519 **rejects a low-order peer key** instead of returning the all-zero
  shared secret it would otherwise produce, and a key exported for one curve
  cannot be imported as the other (the DER OID and the JWK `crv` are both
  checked). Backed by `ed25519-dalek`/`x25519-dalek` on the `sha2` generation
  already in the tree; neither draws its own randomness — a key is built from
  32 bytes of Entropy-provider output, and Ed25519 signing is deterministic, so
  every byte of key material still traces to the injected provider (D9).

- **`crypto.subtle.wrapKey` / `unwrapKey`, the AES-KW algorithm, and `jwk` for
  symmetric keys.** `wrapKey`/`unwrapKey` are required members of
  `SubtleCrypto` and were absent from the prototype entirely — so the one
  operation whose whole point is moving a key without its material ever becoming
  a readable JS value could not be performed at all. Wrapping accepts AES-KW
  (NIST SP 800-38F / RFC 3394, verified against the RFC's published vector),
  AES-GCM, AES-CBC, AES-CTR and RSA-OAEP. AES-KW is reachable **only** through
  wrapping — `encrypt({name:"AES-KW"})` is a `NotSupportedError` — and its
  integrity check makes an unwrap of tampered ciphertext fail rather than hand
  back wrong key material. Wrapping still requires `extractable: true` (wrapping
  is an export) and the wrapping key's `wrapKey` usage.
  AES and HMAC keys now also import and export as `kty: "oct"` JWKs, without
  which `wrapKey("jwk", …)` — the common shape — had nothing to serialize; the
  JWK's `alg` is checked on import, so a key labelled `A128GCM` cannot silently
  become an AES-CBC key.

- **Failures that reach the global scope are now dispatched as events, and an
  exception out of a timer callback is no longer swallowed.** Three gaps closed
  together, because they are one story: what happens to a failure with no code
  left to catch it.
  - **`setTimeout(() => { throw x })` used to vanish.** The throw was caught to
    keep it from unwinding into V8 and then simply dropped — no output, exit 0,
    no trace that anything went wrong. It now fires a cancelable `error`
    (`ErrorEvent`) on the global scope, and `esrun` reports it and exits
    non-zero if nothing claims it.
  - **`unhandledrejection` fires.** `addEventListener("unhandledrejection", …)`
    registered fine and was never called; there was no way for guest code to
    observe a rejection, log it, or suppress it. It now receives a cancelable
    `PromiseRejectionEvent` carrying both `reason` and `promise`.
  - **`rejectionhandled` fires** when a handler is attached to a rejection whose
    report has already gone out. It does **not** retract that report: the
    process still fails, the same stance Node and Deno take.

  `preventDefault()` is how guest code takes responsibility — a claimed failure
  never reaches the embedder. `onerror`, `onunhandledrejection` and
  `onrejectionhandled` are single-handler slots over the same events;
  `PromiseRejectionEvent` is a new global. `TickStatus` gains `uncaught_errors`
  alongside `unhandled_rejections`, and `Driver::run_to_completion` now returns a
  `DriveOutcome` carrying both instead of a bare `Vec<String>` (**breaking** for
  embedders driving the runtime themselves).

- **`navigator.userAgent`.** The WinterTC Minimum Common API requires it and the
  global was missing entirely — while the docs listed `navigator` under "browser
  globals, out of scope", which contradicted the compliance claim. It reports
  `"ES-Runtime/<version>"`, substituted from the crate's own version when the
  prelude is assembled, so the string cannot drift from the binary reporting it
  (a Rust test asserts exactly that). `navigator` is a branded `Navigator`
  instance with `userAgent` on the prototype, and the constructor is not
  callable from a script. **`userAgent` is the whole interface:** the rest of the
  browser `Navigator` is document, device and permission surface, and answering
  those with plausible constants would make a feature check pass and then lie.

### Fixed

- **CI now runs the JavaScript test suite and checks the generated bundle.** The
  pure-JS Protobuf implementation under `crates/runtime/js` has 41 tests that
  nothing ever ran: they are not reachable from `cargo test`, and there was no
  `bun` step, so the largest hand-written JS subsystem in the repo gated on
  nothing. The new `js` job runs `bun test`, then rebuilds the committed
  `runtime:serialization` bundle and requires the tree to be unchanged — the
  bundle is embedded at compile time, so source and artifact could silently
  diverge. The Linux test job also runs the conformance suite through the real
  CLI (`esrun crates/runtime/conformance/run.js`).

- **The conformance gate was silently dropping 22 assertions — it now runs
  278/278, up from a recorded 256.** Four files (`protobuf.js`,
  `serialization.js`, `serialization_edge.js`, `jsonl_test.js`) open with
  `await import("runtime:serialization")`. The engine raises that as a pending
  *dynamic import* — host work the runtime resolves in its async step, not a
  microtask — so ticking alone could never settle them. The runner gave up after
  a fixed tick count and read the tallies anyway, which made every assertion in
  those files **uncounted rather than failing**: the whole
  `runtime:serialization` surface, the largest pure-JS subsystem in the repo,
  was gated by nothing. The runner now drives dynamic imports, supplies an
  in-memory `FileSystem` so the `runtime:fs` pipeline runs without touching a
  disk, and **panics if the queue does not settle** instead of proceeding.
  Confirmed by sabotage: a deliberate failure inside one of those files now
  fails the build. The same silent-give-up existed in `eval_async`, used by
  dozens of tests, and is fixed with it.
- **The conformance suite runs under `esrun` for real.**
  `conformance/RESULTS.md` told readers to verify the uncounted files "under
  `esrun`, not by this number" — which was impossible: the files call harness
  globals that only the Rust runner injected, so running one directly failed
  with `ReferenceError: test is not defined`. The harness moved into
  `conformance/harness.js`, which the Rust gate `include_str!`s and the new
  `conformance/run.js` loads, so the two runners cannot drift. `esrun
  crates/runtime/conformance/run.js` now runs the whole suite over a real event
  loop, a real filesystem and the real CLI, printing a summary and exiting
  non-zero on failure.

- **The async-WebAssembly tests no longer flake under load.** They waited on a
  compile with a fixed 200-tick spin, but a compile lands on V8's *background*
  threads — how many ticks that takes is a property of how busy the machine is,
  not of the runtime, so `cargo test --workspace` (where the other test binaries
  run alongside) could exhaust the count and then assert against a result that
  had simply not arrived. Waiting is now wall-clock bounded and, critically,
  running out of time **panics naming what was awaited** instead of falling
  through silently and reporting the miss as a behaviour failure. Tests only —
  no runtime change.

- **`clearTimeout`/`clearInterval` now release the event loop.** Clearing a timer
  deactivated its callback engine-side but left the entry in the runtime's
  schedule, so `has_pending_work()` stayed true and `next_timer_deadline_ms`
  kept pointing at a firing that would never happen — a driver waited out the
  original delay before the process could exit. `const id = setTimeout(() => {},
  60000); clearTimeout(id);` took the full 60s under `esrun`; it now exits in
  ~9ms. The schedule prunes cleared timers from the front of its queue wherever
  new ones are drained, which is every point guest code could have touched a
  timer. **Process exit time only — no behavior changed:** a cleared timer never
  fired before and still does not, and live timers keep their own deadlines.

## [0.12.0] - 2026-07-28

### Changed

- **`URLPattern` is now spec-conformant: 369/369 on the official WPT suite.**
  Parsing and canonicalization are delegated to the `urlpattern` crate, the same
  way `URL` delegates to `url`. The crate emits each component's regular
  expression as *source* and V8 compiles it, rather than the crate compiling it
  Rust-side — a split that is measurably better on both axes: constructing a
  pattern costs ~18 µs instead of ~640 µs (a 50-route table: 0.65 ms instead of
  35 ms), and it is what makes `ignoreCase` work at all, since `urlpattern`
  0.6.0's Rust-regex backend discards the flags argument. Matching a URL string
  makes no host call: the components come off `URL`. Against the previous
  hand-written parser, `exec()` is ~2x faster and a hot `test()` ~1.3x faster,
  while conformance goes from 63/369 to 369/369. Component regexes compile with
  the `v` (unicodeSets) flag, so set notation inside a custom group works.
  Newly correct behaviour that the old implementation got wrong: `test()`/
  `exec()` accept a `URLPatternInit`, `baseURL` inside a dictionary is honoured
  (and pairing one with a separate base argument is a `TypeError`), components
  are canonicalized (punycode hosts, default ports, percent-encoding), and `?`
  directly after a group in a constructor string is that group's modifier rather
  than the search delimiter.
- **`URLPattern` implements the real pattern syntax.** It was a regex
  escape-and-substitute pass understanding only `:name` and `*`; a custom regex
  group (`/u/:id(\\d+)`), a `?`/`+`/`*` modifier, or a `{…}` group simply
  failed to match, with no error to say why — the worst failure mode for a
  router. It is now a proper lexer and parser for the path-to-regexp dialect the
  standard adopts, compiled per component: named and anonymous groups, custom
  regexes, all three modifiers, `{…}` grouping, and backslash escapes. A group
  preceded by the component's prefix character absorbs it, so `/a/:b?` matches
  `/a` as well as `/a/x` rather than leaving a dangling separator. `hostname`
  groups are bounded by `.` and `pathname` groups by `/`. Unmatched optional
  groups now report `undefined` rather than `""`, `hasRegExpGroups` is exposed,
  the component accessors moved to the prototype, and a malformed pattern throws
  at construction instead of silently never matching.

### Added

- **`URL.createObjectURL()` / `URL.revokeObjectURL()`, and `fetch` serves
  `blob:` URLs.** A `blob:` URL resolves from an in-process store, so fetching
  one needs **no `Net` capability** — nothing leaves the isolate — and never
  reaches the transport. Entries live until revoked: there is no document unload
  to clear them, so a long-lived process must revoke its own URLs.
- **`MessageChannel`, `MessagePort` and `BroadcastChannel`.** This runtime has a
  single agent — no workers, no second realm — so the other side of a channel is
  always in this isolate and delivery is a queued task rather than a cross-thread
  hop. The observable contract still holds: messages are structured-cloned at the
  `postMessage` call (so a later mutation is not seen by the receiver, and a
  non-cloneable value throws at the call site rather than in a detached task),
  delivered asynchronously and in order, and a port buffers until `start()` —
  which assigning `onmessage` does implicitly, while `addEventListener` does not.
  Closing a port disentangles the pair. A `BroadcastChannel` reaches every open
  channel with the same name except itself, and `postMessage` after `close()` is
  an `InvalidStateError`. Transferring a `MessagePort` is not supported (a
  `DataCloneError`); with one agent there is nowhere to transfer it to.
- **The global scope is an `EventTarget`, and `reportError` dispatches.**
  `addEventListener`/`removeEventListener`/`dispatchEvent` now exist on the
  global, with `event.target` reporting `globalThis`. `reportError()` builds a
  cancellable `ErrorEvent` and dispatches it there, falling back to
  `console.error` only when nothing handled it — so `addEventListener("error",
  …)` and the `onerror` slot both work. An `error` listener that itself throws
  is reported straight to the console rather than re-dispatching forever. This
  closes the `reportError` → `ErrorEvent` deferral in SPEC §7.
- **`ErrorEvent` and `ProgressEvent`.**
- **User Timing.** `performance.mark()`, `measure()`, `getEntries()`,
  `getEntriesByType()`, `getEntriesByName()`, `clearMarks()` and
  `clearMeasures()`, with the `PerformanceEntry`, `PerformanceMark` and
  `PerformanceMeasure` interfaces. `measure` accepts both the positional
  mark-name form and the options bag (`start`/`end`/`duration`/`detail`), and
  measuring against a mark that does not exist is a `SyntaxError`. The entry
  buffer is unbounded — there is no navigation to clear it — so a long-lived
  process that marks in a loop must clear its own entries.
- **`ReadableStream.from()`** adapts any iterable or async iterable into a
  stream, pulling one value per unit of demand (so an infinite generator is
  fine) and forwarding cancellation to the iterator's `return` so it can clean
  up.

### Fixed

- An unknown `ReadableStream` `type` now throws `TypeError` rather than
  `RangeError` — the spec treats it as an invalid enum value, not a bad range.
- **`Blob` validates its arguments.** `new Blob(123)` produced an empty blob
  instead of throwing — `Array.from` turned the mistake into silence. A
  non-iterable `blobParts` is now a `TypeError`. An invalid MIME type was echoed
  back verbatim, so `new Blob([], { type: "not a type" }).type` returned the
  garbage rather than `""`; the type is now parsed and dropped if it does not
  match `type/subtype`, in `slice()` as well as the constructor. The
  `endings: "native"` option is honoured, normalising CRLF and CR in string
  parts. `File.webkitRelativePath` exists and reads `""`.
- **`Event` gained its legacy members, and re-entrant dispatch is an error.**
  `cancelBubble`, `returnValue` and `initEvent()` were absent — still normative
  in the DOM standard, and what older libraries feature-detect on. Dispatching
  an event that was already being dispatched recursed until the stack overflowed
  with a `RangeError`; it now throws `InvalidStateError`, and an event can still
  be dispatched again once the first dispatch has finished.
- **`Headers`, `FormData` and `URLSearchParams` return named iterators.**
  `entries()`, `keys()` and `values()` returned bare generator objects, so they
  reported `[object Generator]` rather than `[object Headers Iterator]` and
  friends. Each interface now has a real iterator object inheriting from
  `%IteratorPrototype%` and branded with its interface name.
- **`URL.parse()`, and `URLSearchParams` accepts any iterable.** `URL.parse` —
  the non-throwing way to parse, and the reason `URL.canParse` exists alongside
  it — was missing. `URLSearchParams` treated only an `Array` as a sequence of
  pairs, so `new URLSearchParams(new Map(...))` or a generator silently produced
  an empty object; any iterable now works. It also percent-encoded `*`, which is
  in the urlencoded safe set and must stay literal. Constructor arities are now
  right for `URL` (1), `URLSearchParams` (0) and `Headers` (0).
- **`crypto`, `crypto.subtle` and `performance` are interface instances.** All
  three were plain object literals, so their methods were own properties of a
  singleton rather than prototype members, they stringified as
  `[object Object]`, and the `Crypto`, `SubtleCrypto` and `Performance`
  constructors did not exist to check instances against. Each is now an instance
  of a branded class with its members on the prototype, and the constructors are
  exposed but throw `TypeError` when called, as in browsers. `performance` and
  `crypto` are consequently no longer frozen objects — like every other platform
  object, their behaviour lives on a prototype.
- **Internal plumbing no longer sits on public prototypes.** `Blob.prototype`
  carried `_bytes`, `Headers.prototype` `_list`, `Response.prototype` `_parts`,
  `Event.prototype` `_begin`/`_end`/`_immediateStopped`, and so on — all
  enumerable by `Object.getOwnPropertyNames` and callable by user code. They are
  symbol-keyed slots now. Most are fragment-local symbols and so are genuinely
  unreachable; the three that one prelude fragment defines and another reads
  (`Blob`'s bytes, `FormData`'s encoder, `Response`'s parts) live on a locked,
  non-enumerable `__internal` table, the same defense-in-depth treatment `__ops`
  already gets. As with `__ops`, this is JS-surface hygiene, not the security
  boundary — that stays in the engine's Rust `OpState`.
- **`structuredClone` preserves errors, clones blobs, and supports `transfer`.**
  An `Error` was reconstructed as `new value.constructor(message)`, which lost
  `cause` and `stack` and turned a `DOMException` into a plain `Error` — the
  clone of a rejected value no longer said why it failed. Errors now keep their
  name, `cause` (cloned recursively) and `stack`, with `DOMException`
  reconstructed through its two-argument constructor; a non-standard `Error`
  subclass clones as a plain `Error`, as the spec requires. `Blob` and `File`
  are serializable and now clone by value. The `transfer` option is honoured:
  listed `ArrayBuffer`s are detached and their contents move into the clone,
  and a view onto a transferred buffer is a `DataCloneError` rather than a
  `TypeError` escaping from the copy.
- **`TextDecoder.decode()` honours `{ stream: true }`.** The options argument
  was ignored outright, so a decoder fed a multi-byte code point split across
  two calls produced replacement characters instead of the character — the
  classic corruption when decoding a byte stream chunk by chunk. The decoder now
  holds back a trailing incomplete sequence until the next call and flushes it
  on the final, non-streaming `decode()`. An invalid lead byte is not a prefix
  of anything and is decoded immediately rather than held. A BOM is stripped
  once at the start of a stream rather than at every chunk boundary.
  `TextDecoderStream` now delegates to this instead of carrying a second copy of
  the same buffering logic.
- **`Request` and `Response` gained `formData()`, and `Request` its policy
  members.** `formData()` did not exist on either body — a `multipart/form-data`
  upload could be *sent* but never *received*, which is the shape a server
  runtime needs most. Both now parse `multipart/form-data` (locating parts by
  the CRLF-prefixed delimiter, so a boundary sequence inside a payload is not
  mistaken for one) and `application/x-www-form-urlencoded`, returning
  `FormData` with `File` entries for parts carrying a filename. `Request` also
  now rejects a body on `GET`/`HEAD` with `TypeError`, and reports `redirect`,
  `cache`, `credentials`, `mode`, `referrer`, `referrerPolicy`, `integrity`,
  `keepalive` and `destination` instead of `undefined` — these are browser
  policy knobs a server runtime has no origin or cache to apply, so they are
  recorded and reported faithfully rather than acted on, since calling code
  branches on their values.
- **`Response` validates its status, and the missing statics landed.** The
  constructor accepted any status at all — `new Response("x", { status: 999 })`
  built a response no server could send, and a body on a null-body status
  (204/205/304) was silently kept. Both now throw (`RangeError` and `TypeError`
  respectively), matching Fetch. `Response.redirect()` was missing entirely and
  now exists, defaulting to 302 and rejecting non-redirect statuses.
  `Response.error()` reported `type: "default"`; it now reports `"error"`, and
  `type` is carried through `clone()`. `Response.json()` never set
  `application/json`, because the serialized string body had already inferred
  `text/plain` — it now sets the JSON type unless the caller's `init` supplied
  one, and throws `TypeError` for a value `JSON.stringify` cannot represent.
  Responses the runtime builds itself (network responses, `Response.error()`)
  are *internal* responses in Fetch terms and bypass the constructor checks, so
  a real 204 from a server still works.
- **`fetch` honours `AbortSignal`.** `Request.signal` did not exist and the
  `signal` option was dropped on the floor, so a request could not be cancelled
  and `AbortSignal.timeout` had no effect on it — there was no way to bound an
  outbound call. `Request` now exposes a `signal` (defaulting to a fresh
  unaborted one, adopted from `init.signal` or from the request being cloned),
  and `fetch` rejects with the signal's reason: immediately if the signal is
  already aborted, without touching the transport. An abort mid-flight is real
  cancellation, not a flag — new `fetch_abort_new`/`fetch_abort` ops race the
  transport future against the signal and **drop** it when the signal wins,
  tearing down the connection. An abort after the response headers arrive errors
  the body stream and drops the host-side stream via `fetch_body_cancel`, which
  is also now wired to `ReadableStream.cancel()` so an abandoned body no longer
  holds its connection open. The abort is wired before the first suspension
  point, so a signal firing while the request body is still being materialized
  is not missed.

- **`Headers` reject values containing NUL, CR or LF.** Values were trimmed of
  surrounding whitespace but never validated, so a CR/LF *inside* a value —
  reachable from any header built out of untrusted input — could splice extra
  header lines or a body into the wire format (request/response splitting).
  Setting such a value now throws `TypeError`, as the Fetch standard requires.
- **Platform objects are branded with `Symbol.toStringTag`.** Every class-based
  Web API interface — `Blob`, `File`, `FormData`, `URL`, `URLSearchParams`,
  `Event` and friends, `AbortController`/`AbortSignal`, `TextEncoder`/
  `TextDecoder`, the whole streams family, `Headers`/`Request`/`Response`,
  `CompressionStream`/`DecompressionStream`, `URLPattern`, `WebSocket` — now
  reports its interface name from `Object.prototype.toString` instead of
  `[object Object]` (WebIDL §3.7.5). Type-sniffing libraries, test-framework
  diffs and inspector output all key on this. The tag is a non-enumerable,
  non-writable, configurable property of the prototype, as WebIDL specifies.
  `crypto`, `crypto.subtle` and `performance` are plain object literals rather
  than class instances and are still unbranded.

### Added

- **Known deviations are now executable in the conformance suite.** The harness
  gains `todo(name, fn)` alongside `test(name, fn)`: a `todo` states what the
  spec requires while the runtime is known not to satisfy it. Throwing `todo`s
  are tallied separately and do not fail the build, but a `todo` that *passes*
  does — so a fix cannot land without being promoted to `test` and locked in
  against regression. An audit of the Web API surface against WebIDL and the
  WHATWG specs added **46** such entries plus **28** new passing assertions,
  across four new files (`webidl.js`, `blob.js`, `fetch.js`, `timers.js`) and
  extensions to the encoding, URL, event, structured-clone, performance and
  stream files. The count moves to **125/125 passing, 46 known deviations**; see
  [conformance/RESULTS.md](crates/runtime/conformance/RESULTS.md) for the
  grouped list.

### Changed

- The internal `fetch` op's argument layout gains `abortId` after
  `bodyStreamId`; headers now start at index 5. New ops: `fetch_abort_new`,
  `fetch_abort`, `fetch_body_cancel` (all `Capability::Net`).

## [0.11.0] - 2026-07-21

### Added

- **`/docs/wasm` site page.** One place for the whole WebAssembly + WASI surface:
  the JS API, `.wasm` ES module imports, a proposal matrix measured with
  `wasm-feature-detect` against Node/Bun/Deno (esrun matches Deno exactly — the
  surface is V8's), the WASI member and syscall tables, the three-check
  filesystem model, and the caveats (no running wasm threads without Workers,
  buffering streaming compiles, no `import source`, blocking WASI file calls).
  The `WebAssembly` and `runtime:wasi` sections in
  [Global objects](https://esrun.opentechf.org/docs/globals) and
  [Module system](https://esrun.opentechf.org/docs/modules) now link there
  instead of restating it, and the comparison table gains WebAssembly, `.wasm`
  module import, and WASI rows.
- **wasm/WASI benchmarks.** Five cross-runtime workloads covering the new
  surface: `wasm_compile` (validation + codegen), `wasm_call` (the JS↔wasm
  boundary vs execution inside wasm), `wasm_mem` (the shared linear-memory
  interop shape), `wasi_start` (what running a `wasm32-wasip1` program costs per
  invocation), and `wasi_syscall` (the preview-1 implementation, called from
  inside the guest). Modules are assembled in JS (`bench/scripts/wasm-mod.js`)
  rather than checked in as `.wasm` fixtures, so every runtime compiles
  byte-identical input and the compile workload can vary a constant per iteration
  to defeat compilation caches. `WASI` resolves from `runtime:wasi` on esrun and
  `node:wasi` on Node/Bun/Deno; LLRT has no `WebAssembly`, so all five are n/a
  there. esrun leads `wasi_start` and `wasm_call`; `wasm_compile` is ~4× behind
  Deno on the same engine, which the synchronous-compile control isolates to wasm
  codegen in the prebuilt `rusty_v8` — the same attribution as the `compute` row.
  See [bench/README.md](bench/README.md).

- **WebAssembly.** The `WebAssembly` JS API now works end-to-end. V8 always
  supplied the namespace, but the promise-returning entry points could never
  settle: V8 compiles off-thread and reports completion as a *foreground* task,
  and nothing drained that queue — so `await WebAssembly.compile(bytes)` hung
  forever. The loop now pumps V8's task queue each tick, and tracks in-flight
  compiles as pending work so a driver neither exits nor parks mid-compile.
  `compileStreaming` and `instantiateStreaming` are added (absent from bare V8,
  being defined by the fetch integration); they take a `Response` or a promise
  for one, enforce a `application/wasm` Content-Type and an ok status, and
  currently buffer before compiling. WebAssembly needs no capability — a module
  is exactly as privileged as the import object it is given. The ESM integration
  (`import ... from "./m.wasm"`) is still unsupported. See
  [API.md](docs/API.md#webassembly); 18 new conformance assertions.

- **WebAssembly ES-module integration.** `import { add } from "./add.wasm"` now
  works — the last gap that kept `.wasm` from being a first-class module here,
  and something Node, Deno and Bun all ship unflagged. A wasm import's *module*
  half is an ordinary specifier resolved through the same graph as any `import`,
  so `(import "./env.js" "log" …)` takes `log` from that module's namespace;
  exports become the module's exports, including names that are not JS
  identifiers. One instance per graph, shared by static and dynamic imports; a
  malformed `.wasm` fails at load with V8's own diagnostic.
  Source-phase imports (`import source`) are still unsupported.

- **`runtime:wasi` — WASI preview 1.** Enough of the
  `wasi_snapshot_preview1` ABI to run what the `wasm32-wasip1` toolchains emit
  for compute-and-print workloads: arguments, environment, clocks, randomness,
  stdio and process exit. `new WASI({ args, env })` →
  `getImportObject()` → `start(instance)`, which returns the exit status (`0` on
  a normal `_start` return, otherwise the `proc_exit` code) while letting a real
  fault propagate.

  **Arguments and environment come only from the constructor.** Unlike Node's
  `node:wasi`, there is no path by which a wasm module reads the host's real
  environment through this API, so constructing one needs no capability and
  inherits nothing — forwarding the real environment is an explicit, visible act
  through the `Env`-gated `runtime:process`. Node's docs are careful to say its
  threat model "does not provide secure sandboxing" and that WASI capabilities
  there "do not form a security model"; here the sandbox is the runtime's own.

  Stdout/stderr are line-buffered through the console sink with the trailing
  partial write flushed at exit; stdin reads as end-of-file.
  See [API.md](docs/API.md#runtimewasi).

- **WASI filesystem, behind three checks.** `new WASI({ preopens: { "/sandbox":
  "./data" } })` gives a guest a directory, and `path_open`, `fd_read`,
  `fd_write`, `fd_seek`/`fd_tell`, `fd_readdir`, `fd_filestat_get`,
  `path_filestat_get`, `path_create_directory`, `path_unlink_file`,
  `path_remove_directory` and `path_rename` all work against it.

  Reaching a file passes **three independent checks**: the preopen must map it
  (and `../` cannot climb out, so two preopens stay isolated), the host op must
  hold `FileRead`/`FileWrite`, and the provider's root jail must contain the
  resolved path. This is the sandboxing Node's `node:wasi` documentation
  explicitly disclaims.

  Calls that have no host primitive yet (`fd_pread`/`fd_pwrite`, `path_link`,
  `path_symlink`, `path_readlink`, the `*_set_times`/`set_size` calls, sockets)
  report `ENOTCAPABLE`, and every import stays *present* — a missing one is a
  `LinkError` for a program that merely links the symbol.

- **`SyncFileSystem` provider + synchronous filesystem ops.** A new provider
  seam for callers that cannot await, with `SystemSyncFileSystem` as the
  OS-backed default (same base and same root jail as `SystemFileSystem`), wired
  through `HostProviders::with_sync_file_system`. It exists because WASI's
  syscalls are synchronous — a guest calls `fd_read` and expects bytes back with
  no chance to yield — so the async `FileSystem` cannot serve them however the
  ops are arranged. These are the only sync I/O ops in the runtime and they
  block the runtime's thread for the duration of the call; an embedder that
  cannot afford that installs no implementation, and WASI reports `ENOTCAPABLE`.

### Fixed

- **`@opentf/esrun-types` shipped an unresolvable reference.** `index.d.ts`
  references `runtime-serialization.d.ts`, but the package's `files` list omitted
  it, so the published tarball lacked the file. Added, along with the new
  `runtime-wasi.d.ts`.

### Changed

- **Breaking (embedders implementing `ModuleLoader`):** `load` now returns
  `ModuleSource` — `Text(String)` or `Wasm(Vec<u8>)` — rather than `String`, so
  the seam can carry a binary module at all. A text-only loader wraps its string
  in `ModuleSource::Text`. `Engine` also gains `compile_wasm`, breaking for
  out-of-repo implementors of that trait.

## [0.10.0] - 2026-07-20

### Security

- **Dependency updates clear all outstanding RUSTSEC advisories.**
  `quick-xml` (the `runtime:serialization` XML backend) is bumped to 0.41,
  fixing two high-severity guest-reachable DoS advisories — quadratic
  duplicate-attribute checking (RUSTSEC-2026-0194) and unbounded
  namespace-declaration allocation (RUSTSEC-2026-0195). `self_update` (the
  `esrun upgrade` backend) moves to 1.0.0-rc.6, dropping its vulnerable
  quick-xml 0.37 tree (renamed features/methods adopted; behavior unchanged).
  A workspace-wide `cargo update` clears the remaining lockfile advisories
  (`quinn-proto` RUSTSEC-2026-0185, `anyhow` RUSTSEC-2026-0190, yanked
  `spin` 0.9.8). `cargo deny check` and `cargo audit` are green again; the
  only remaining allowance is the documented `rsa` Marvin ignore.

### Testing / CI

- **Cross-platform test CI.** The behavioral test job now runs on a
  **Linux + Windows + macOS** matrix (was Linux-only), so platform-divergent
  surfaces — filesystem/path semantics, the symlink-canonicalized root jail,
  process exit codes, networking, CRLF/encoding — are covered on each tier-1 OS.
- **Soak / leak tests.** Opt-in soak tests (`#[ignore]`,
  `cargo test -- --ignored soak`) hammer a subsystem over many iterations and
  assert it neither leaks nor deadlocks. The first runs 20k streaming-`fetch`
  uploads and asserts the request/response body registries drain to zero every
  iteration (precise native-leak guard) with bounded steady-state RSS.

### Added

- **Stable guest-facing error codes** (SPEC §6 Phase 13 — now complete).
  Host-side failures carry a stable string `code` on the thrown JS exception
  (`e.code === "ERR_NOT_FOUND"`), the contract guest code branches on;
  messages stay human prose. The documented set (API.md §Error codes) covers
  capability denials (`ERR_CAPABILITY_DENIED`), missing providers
  (`ERR_PROVIDER_UNAVAILABLE`), filesystem io kinds (`ERR_NOT_FOUND`,
  `ERR_ALREADY_EXISTS`, `ERR_PERMISSION_DENIED`, …), the root jail
  (`ERR_JAIL_ESCAPE`), and networking (`ERR_CONNECTION_REFUSED`,
  `ERR_TIMED_OUT`, `ERR_DNS`, `ERR_TLS`, …). Plumbing: a new
  `ErrorCode` enum in `common`, `IntoException::exception_code`, an
  `OpError::with_code` builder, and a **new `ProviderError::Coded` variant**
  (+ `ProviderError::from_io`) that default providers use to classify io/TLS/
  DNS failures (**breaking** only for embedders exhaustively matching
  `ProviderError`; it is `#[non_exhaustive]`). The engine defines `code` as an
  own data property so it also lands on `DOMException`s (shadowing the legacy
  numeric getter), and `runtime:net`'s `SocketError:` rewrap preserves it. An
  error with no stable classification simply carries no `code`.

- **Compression Streams.** `CompressionStream` and `DecompressionStream`
  (WinterTC Minimum Common API) ship as prelude globals for all four spec
  format tokens: `brotli`, `gzip`, `deflate` (zlib), and `deflate-raw`. Each
  stream is a `TransformStream` over a stateful native codec context — flate2,
  plus the pure-Rust `brotli` crate (encode quality 5, the balanced streaming
  default; window 22) — behind pure sync ops (no capability): chunks stream
  through with whatever output the codec produces, and errors follow the spec —
  corrupt input and trailing junk reject at write, a truncated stream rejects
  at close, all as `TypeError` (the zlib/raw decoders run on the low-level
  `Decompress` state machine to catch truncation the write adapters miss;
  brotli's adapters were verified to detect all three cases themselves).
  Brotli output is verified against Node's Google-C decoder, gzip against
  system gzip.
  Alongside this, the handwritten streams got two spec-correctness fixes:
  **`transformer.cancel`** now runs (once) on writable-abort / readable-cancel —
  Compression Streams use it to free the native context — and all
  source/sink/transformer methods are invoked with **promise-calling
  semantics**, so a synchronous throw becomes a rejection instead of unwinding
  through the stream machinery as an unhandled rejection that left the write
  promise permanently pending.

- **`WebSocketStream`.** The promise/stream-based WebSocket interface from the
  WHATWG spec ships as a prelude global alongside the classic `WebSocket`, over
  the same connection ops and `Net` capability gate. `opened` resolves to
  `{ readable, writable, protocol, extensions }`: reads are pull-based (one
  host receive per pull — real receive backpressure) and each write resolves
  when the host has taken the frame (send backpressure); strings travel as text
  frames, `BufferSource`s as binary. `closed` settles with
  `{ closeCode, reason }` on a clean close and rejects with the new
  **`WebSocketError`** (a `DOMException` subclass carrying `closeCode`/`reason`,
  also a global) on failure. `close({ closeCode, reason })` validates like the
  classic interface; an `AbortSignal` option drops the connection. After a
  local close an internal drain keeps receiving until the peer's close frame,
  so `closed` settles even with no active reader.

- **Streaming `runtime:http` server bodies.** The HTTP server now streams bodies
  in **both** directions instead of buffering them. The handler's `Request` body
  is a `ReadableStream` pulling chunks from the host as they arrive on the wire
  (nothing is materialized unless the handler asks, e.g. `request.text()`), and a
  `Response` with a `ReadableStream` body is sent with chunked transfer-encoding
  as the guest produces it — pumped one chunk at a time across a bounded channel
  (download backpressure), so an SSE-style or open-ended response never
  materializes. The proxy/echo shape `new Response(request.body)` pipes inbound
  to outbound with nothing buffered. A buffered (string/bytes) body still crosses
  inline as the fast path. New provider type `HttpServerBody`
  (`Empty`/`Bytes`/`Stream`) replaces `Vec<u8>` on both `HttpServerRequest.body`
  and `HttpServerResponse.body` (**breaking** for embedders implementing
  `HttpServerProvider`); `SystemHttpServer` hands off hyper's request body as a
  chunk stream and writes streamed responses via `StreamBody` (DECISIONS D31).

- **Streaming `fetch` request bodies.** A `fetch` whose body is a `ReadableStream`
  now uploads with chunked transfer-encoding instead of being buffered first, so a
  large or open-ended request body streams to the server with bounded memory.
  The guest stream is pumped into the host one chunk at a time across a bounded
  channel (upload backpressure); a stream error aborts the in-flight request, and
  a non-stream body (string/bytes/Blob/FormData) still travels buffered as before.
  New provider type `RequestBody` (`Empty`/`Bytes`/`Stream`) replaces
  `HttpRequest.body: Option<Vec<u8>>` (**breaking** for embedders implementing
  `NetTransport`). This closes the last Fetch streaming gap — request **and**
  response bodies now stream (SPEC §2.9; DECISIONS D20). New cross-runtime
  benchmark **`fetch_upload`** (200 streamed POSTs): the workload verifies the
  bytes actually arrived, so a runtime that doesn't truly stream the body is
  recorded n/a; esrun ties Deno and leads Bun/Node, lowest RSS.

- **Protobuf proto3-JSON.** `schema.toJson(messageName, value)` and
  `schema.fromJson(messageName, json)` convert between the decoded value shape
  and the canonical proto3-JSON mapping: 64-bit integers and `bytes` as strings
  (base64 for `bytes`), enums as value-names, and the well-known-type special
  forms (Timestamp/Duration as strings, wrappers as bare values,
  Struct/Value/ListValue as native JSON, Any with an `@type` member, FieldMask
  as a comma path string, Empty as `{}`).
- **Protobuf edition 2024.** `new Protobuf.Schema(proto)` now accepts
  `edition = "2024"` in addition to proto3 and edition 2023. The 2024 defaults
  for the wire-affecting features (field presence, repeated encoding, enum type)
  match edition 2023.
- **Protobuf descriptor-set loading.** `Protobuf.Schema.fromDescriptorSet(bytes)`
  builds a schema from a compiled `FileDescriptorSet` (`protoc
  --descriptor_set_out`, ideally `--include_imports`) instead of `.proto`
  source — the common way production systems distribute schemas. proto3 and
  editions 2023/2024 only; encodes byte-identically to a text-built schema.
- **Protobuf length-delimited framing.** `schema.encodeDelimited(messageName,
  value)` writes one varint-length-prefixed message (the `writeDelimitedTo`
  framing); `schema.decodeDelimited(messageName, source)` streams such messages
  back from a `ReadableStream`, async/sync iterable, or `Uint8Array`.
- **Protobuf streaming.** `schema.decodeStream(messageName, fieldName, source)`
  is an async generator that streams the elements of a repeated message field
  from a chunked byte source (a `ReadableStream`, async/sync iterable of
  `Uint8Array`, or a `Uint8Array`), decoding each element as it arrives and
  skipping the outer message's other fields — so a large collection is
  processed without materializing the whole array.
- **Protobuf delimited (group) message encoding.** Editions
  `features.message_encoding = DELIMITED` message fields now decode/encode as
  groups instead of being preserved as opaque unknown fields, so editions JSON
  output is lossless. The official conformance suite (v29.3,
  `--maximum_edition 2023 --enforce_recommended`) now passes **4101 successes,
  0 unexpected failures** (one proto2-extension-in-JSON case is a documented
  expected failure — proto2 extensions are unsupported by design).
- **Protobuf proto2 schemas.** `new Protobuf.Schema(proto)` and
  `Protobuf.Schema.fromDescriptorSet(bytes)` now accept `syntax = "proto2"`
  (and descriptor sets with an unset `syntax`), covering the large legacy
  proto2 ecosystem. proto2 maps onto the editions feature machinery: explicit
  field presence (so `required`/`optional` keep zero values on the wire),
  unpacked repeated fields by default, and closed enums (an unrecognized value
  is retained as an unknown field). `group` fields lower to delimited message
  fields and round-trip with start/end-group framing. `required` is mapped to
  explicit presence but **not enforced** as present; custom field defaults
  (`[default = …]`) parse but are **not materialized** (the decoded value shape
  stays sparse). **Extensions** (`extend`) remain unsupported — extension
  fields continue to round-trip losslessly as preserved unknown fields. `group`
  and `required` are rejected outside proto2.

### Fixed

- **`esrun upgrade` and the install scripts work with the new release asset
  naming.** The release process moved to the `otf-release` tool, which names
  assets `esrun-<os>-<arch>.{tar.gz,zip}` (e.g. `esrun-linux-x86-64.tar.gz`)
  with the binary at the archive root — but `esrun upgrade`, `install.sh`, and
  `install.ps1` still expected the old `esrun-<version>-<rust-triple>` names and
  a nested binary, so `esrun upgrade` failed with *"No asset found for target"*
  and the installers 404'd. All three now target the new names; checksum
  verification is skipped (with a note) when a release ships no `checksums.txt`.
  `esrun upgrade` additionally ran `self_update`'s blocking HTTP runtime inside
  the async `main`, panicking on runtime drop — it now runs on a dedicated OS
  thread.
- **Dynamic `import()` of a module with a syntax error rejects with a
  `SyntaxError`.** It previously rejected with a generic `Error`, so a `.catch`
  that inspects `error.name` saw `"Error"` rather than `"SyntaxError"`. The
  failure's class is now threaded through the dynamic-import rejection path
  (missing modules still reject with `Error`). Surfaced by tc39/test262
  `dynamic-import/catch/*`.
- **Dynamic `import()` of an errored module cycle no longer crashes the
  process.** Dynamically importing a member of an async (top-level-await) cycle
  whose evaluation already threw re-evaluated an errored module, tripping a V8
  `CHECK` and aborting the whole runtime (`SIGABRT`) — guest-triggerable. V8
  exposes no safe per-module way to detect this (a failed async cycle member
  reports `Evaluated`, and `Evaluate`/`GetException` abort on it), so the engine
  now tracks graph adjacency and propagates an evaluation failure across the
  reachable graph; a re-import of an errored member rejects with the recorded
  error instead of re-evaluating. Surfaced by tc39/test262
  `dynamic-import/import-fulfilled-member-of-errored-cycle`.
- **Caught dynamic `import()` failures no longer reported as unhandled
  rejections.** Dynamically importing a module that throws at top level and
  catching it (`await import('./throws.js').catch(...)`) still reported the
  module's internal evaluation promise as an unhandled rejection and exited
  nonzero. The engine observes that promise by polling and forwards its outcome
  to the `import()` promise, so it is now marked handled — the guest's `.catch`
  handles the failure.
- **Dynamic `import()` rejection reactions no longer silently dropped.** When a
  dynamic import failed to load (missing module, or a module with a syntax
  error), its promise was rejected *after* the tick's microtask checkpoint, so
  the queued `.catch`/rejection reaction never ran if the event loop then went
  idle — `import('./missing.js').catch(...)` completed with exit 0 and the
  handler never fired. Rejections are now deferred into the tick (before the
  checkpoint), symmetric with the resolve path, so reactions always run.
  Surfaced by tc39/test262 `dynamic-import/catch/*`.
- **Protobuf decode recursion limit.** `decode` (and the streaming/group skip
  paths) now bound message nesting to a maximum depth (100, the protobuf
  default), rejecting deeply-nested sub-messages or groups instead of exhausting
  the JS stack — hardening against hostile input on the binary decode path.
- **Protobuf CLOSED-enum decoding.** An unrecognized number in a CLOSED enum
  (proto2/editions `features.enum_type = CLOSED`) is now retained as an unknown
  field (lossless on re-encode) rather than surfaced as the field value; open
  enums (proto3 default) keep the existing pass-through behavior. The resolved
  `closed` flag was previously computed but unused by the decoder.

### Changed

- **`Protobuf.Schema` methods renamed `parse`/`build` → `decode`/`encode`**
  (breaking) to match the binary `MessagePack` namespace (`decode`/`encode`).
  No aliases are kept. The text namespaces (`XML`/`YAML`/`TOML`) keep
  `parse`/`build`.

## [0.9.0] - 2026-06-23

### Added

- **`runtime:serialization` — `Protobuf`.** A pure-JS, reflective Protobuf
  implementation (no native deps, no codegen). `new Protobuf.Schema(proto)`
  compiles a `.proto` source string (or a `{ filename: source }` map) at runtime
  — proto3 and edition 2023; proto2-only constructs are rejected — and
  `schema.parse(messageName, bytes)` / `schema.build(messageName, value)` decode
  and encode the binary wire format. Decoded objects use camelCase keys, BigInt
  for 64-bit ints, enum value-names, and `Uint8Array` for bytes; unknown fields
  are preserved across re-encode. Passes the **official protobuf conformance
  suite** for binary wire format (2060 successes, 0 failures across proto3 +
  edition 2023; JSON/text-format/proto2 out of scope); byte-for-byte verified
  against protobuf-es.

### Changed

- **`runtime:parsers` renamed to `runtime:serialization`** (breaking). One module
  name now covers both text and binary serialization formats; the exported
  namespaces (`XML`/`YAML`/`TOML`/`JSONL`/`MessagePack`) and their APIs are
  unchanged — only the import specifier moves.
- **Protobuf benchmark now compares each runtime's real path** instead of forcing
  protobuf-es on every runtime: esrun decodes with its native
  `runtime:serialization` Protobuf, Node/Bun/Deno with protobuf-es. esrun leads on
  both time and memory (small 77ms/37MB, large 4.1s/108MB).

### Fixed

- **`runtime:serialization` — TOML datetimes** now parse to RFC3339 strings instead of
  leaking the `toml` crate's internal `$__toml_private_datetime` round-trip
  sentinel object.
- **`runtime:serialization` — YAML non-finite floats** (`.inf`/`.nan`) now parse to
  `Infinity`/`NaN` instead of being silently coerced to `null`. YAML and TOML
  parsing build engine values directly rather than transcoding through JSON,
  which JSON's number model cannot represent.

## [0.8.0] - 2026-06-20

### Added

- **`runtime:parsers` Module** — native parsers exposed as namespace objects
  (`XML`, `YAML`, `TOML`, `MessagePack`, `JSONL`).
  - `YAML` / `TOML`: `parse`, `build`, and `validate` (`{ detailed }` for
    `{ valid, error }`).
  - `MessagePack`: `decode`, `encode`, and `validate` for binary data.
  - `JSONL`: `DecoderStream` (with `skipInvalid`) and `EncoderStream` transform
    streams for robust pipeline streaming of JSON Lines.
  - Parsers are backed by optimized Rust implementations over the host op seam.

### Changed

- **`runtime:parsers` performance optimization.** The YAML, TOML, and MessagePack parsers now use `serde_transcode` to stream directly to JSON strings, bypassing the slow C++ FFI object construction, resulting in up to 2.5x parsing speedups and huge memory usage reductions.
- **`runtime:net` — full WinterTC Sockets conformance.** `Socket.close(reason?)`
  now accepts the spec's optional advisory `reason` argument (ignored by the
  transport), and socket failures — invalid options, connect/TLS/I/O errors —
  surface as a `TypeError` whose message is prefixed `"SocketError: "` (the
  WinterTC `SocketError` shape). This closes the last letter-of-the-spec gaps;
  the `runtime:net` surface now fully matches the
  [WinterTC Sockets proposal](https://sockets-api.proposal.wintertc.org/).

## [0.7.0] - 2026-06-19

### Added

- **`esrun --env-file <path>` — `.env` loading** (DECISIONS D30). Loads
  environment variables from a single `.env` file into `runtime:process` `env`.
  **No auto-discovery** — a file is read only when the flag is passed. The OS
  environment wins on a conflict by default;
  `--env-override` lets file values win instead. The real process environment is
  never mutated. The parser is a fixed, documented dialect (quoting + escapes,
  `#` comments, `export ` prefix, BOM/CRLF) with **no variable expansion**.
- **`runtime:process` secret masking** (DECISIONS D30). Env values with a
  secret-bearing key (case-insensitive: ending in `_KEY(S)`, `_TOKEN(S)`,
  `_SECRET(S)`, `_PASS`, `_PASSWORD(S)`, or containing `CREDENTIAL`/`AUTH`) are
  exposed as an opaque `Secret` that renders as `"[redacted]"` in `console`, string
  coercion / template literals, and `JSON.stringify`, guarding against
  accidental leakage. The new `unmask(value)` export reveals the real value
  (plain strings pass through unchanged); `runtime:process` also exports
  `Secret`.

## [0.6.0] - 2026-06-18

### Added

- **`runtime:net` server-side TLS termination.** `listen({ secureTransport:
  "on", cert, key, alpn })` now terminates TLS: pass a PEM certificate chain +
  private key and every accepted `Socket` is encrypted, with the negotiated
  protocol in `opened.alpn`. The cert/key are supplied inline by the guest, so
  server TLS needs no capability beyond `NetListen` (the bind's). The default
  `SystemNet` builds the rustls `ServerConfig` once at bind time and runs each
  handshake concurrently in the accept task, so a slow client can't block other
  connections (DECISIONS D28). This closes the last `runtime:net` TLS gap —
  client `connect`, `startTls`, and server `listen` are all implemented.
- **`runtime:net` `startTls()`.** A socket opened with
  `connect(addr, { secureTransport: "starttls" })` now starts in plaintext and
  can be upgraded to TLS in place with `socket.startTls()` (the SMTP/IMAP/XMPP
  STARTTLS shape) — it returns a new `Socket` for the encrypted stream (SNI +
  ALPN, certificate verification on), and `upgraded` is `true`. The default
  `SystemNet` reclaims the raw stream from its reader/writer tasks to wrap it,
  replaying any bytes the peer sent before the handshake. Calling `startTls()`
  on a non-`"starttls"` socket throws (DECISIONS D28).
- **`runtime:net` `allowHalfOpen`.** `connect(addr, { allowHalfOpen: true })`
  keeps the writable usable after the peer's FIN (read EOF), instead of tearing
  the whole socket down. Default stays `false` (WinterTC).

### Fixed

- **`runtime:net` listener close cancels a parked `accept`.** Closing a
  `Listener` while an `accept()` was waiting (e.g. a detached
  `for await (conn of server)` loop closed from elsewhere) left the accept
  parked forever, so the loop never ended and the process hung. `close()` now
  aborts the accept task, resolving the parked `accept` to `null` so the loop
  terminates cleanly.

### Changed

- **`runtime:net` `SocketInfo` addresses.** `opened`'s `remoteAddress` /
  `localAddress` are now WinterTC `"host:port"` strings (IPv6 host bracketed)
  rather than bare hosts; `remotePort` / `localPort` remain as a convenience.

- **`runtime:net` TLS connector reuse.** The default `SystemNet` built a fresh
  rustls `ClientConfig` (re-parsing the whole root store) on every secure
  `connect`. Connectors are now memoized by their offered ALPN set and shared
  across connections, so repeated `secureTransport: "on"` connects no longer
  rebuild TLS state.

## [0.5.0] - 2026-06-18

### Added

- **`WebSocket`.** The classic WHATWG `WebSocket` interface ships as a global
  (like `fetch`): `ws:`/`wss:`, `send`/`close`, `binaryType`, `bufferedAmount`,
  `protocol`/`extensions`, and `open`/`message`/`error`/`close` events. Opening
  a connection requires the `Net` capability. The default transport is
  `tokio-tungstenite`; `wss:` reuses the rustls TLS stack. `MessageEvent` and
  `CloseEvent` are now globals too. `WebSocketStream` and permessage-deflate are
  not yet supported (DECISIONS D29).
- **`runtime:websocket` server.** `serve({ hostname?, port })` binds a WebSocket
  server (`ws:`, `NetListen`) and yields accepted connections as an async
  iterable; each connection has the client `WebSocket`'s `send`/`close`/
  `binaryType` + `message`/`close` events. `broadcast(connections, data)` fans a
  message out to many connections in one host crossing (concurrent enqueue +
  coalesced writes — full delivery). A `wss:` server and pub/sub topics are
  follow-ups (DECISIONS D29).
- **Benchmarks.** Added a `websocket` workload (client ping-pong round-trips) and
  a `bench/websocket-chat/` broadcast chat benchmark (server + client sweeps,
  messages/sec).

### Fixed

- **`runtime:parsers` XML nesting depth.** The recursive XML reader descended one
  stack frame per element, so a deeply nested document (`<a><a>…`) could overflow
  the stack and abort the process. Nesting is now bounded (256 levels, matching
  libxml2) and over-deep input fails gracefully with a parse error. A nested
  parse error is also propagated now instead of being silently swallowed.
- **`XMLDecoderStream` unbounded buffer.** The streaming decoder retained bytes
  until a top-level element closed and re-scanned the tail on every chunk, so an
  element that never closed grew memory without bound (and made the re-scan
  quadratic). The retained buffer is now capped (64 MB); past it the stream
  fails with a `RangeError` instead of consuming unbounded memory.
- **`runtime:parsers` error signaling.** `XMLParser.parse`/`XMLBuilder.build`
  detected failures by string-matching a `"Parse failed:"`/`"Build failed:"`
  prefix on the result, so a document that legitimately parses to a string with
  that prefix was mistaken for an error. Parse/build failures now throw
  (`SyntaxError`/`TypeError`) out of band; a string result is always genuine
  data.
- **JSON modules honor the import attribute.** JSON transpilation was triggered
  by a `.json` file extension and ignored the `with { type: "json" }` attribute,
  so a `.json` imported without the attribute was silently accepted and a JSON
  resource without that extension could not be imported at all. Import attributes
  are now plumbed through the engine (static and dynamic imports); transpilation
  is keyed on `type === "json"` regardless of extension, matching the
  import-attributes proposal.
- **`URL` host setter, empty port.** Setting `url.host = "example.com:"` (a
  trailing colon with no port) cleared the existing port; per WHATWG an empty
  port component leaves the port unchanged (verified against the reference
  implementation). Only an explicit, valid port now changes it.

## [0.4.0] - 2026-06-17

### Added

- **JSON modules**: Fully support ES module `import ... with { type: "json" }` for importing JSON data securely, via safe runtime transpilation (no unsafe JS evaluation).
- **`runtime:parsers` Native XML parsing.** The `runtime:parsers` module exposes `XMLParser`, `XMLBuilder`, and `XMLValidator` mapped directly to the `quick-xml` Rust engine. Provides ultra-fast native XML-to-JSON and JSON-to-XML conversion, and strict validation. Now includes **`XMLDecoderStream`**, a native `TransformStream` for incremental, memory-efficient streaming XML parsing that yields native JS objects (`for await (const node of stream)`). 
- **Benchmarks.** The newly added `xml_small` and `xml_large` benchmarks show ES-Runtime drastically outperforming other JS engines (often 2-4x faster than Node/Bun/Deno) via deep structural conversion bypassing `JSON.parse`.

- **`runtime:net` TLS client.** `connect(address, { secureTransport: "on" })`
  now negotiates TLS (rustls via tokio-rustls, `aws-lc-rs`, bundled Mozilla
  roots) with certificate verification. Follows the WinterTC Sockets API: `sni`
  overrides the server name (default: the host), `alpn` offers protocols and the
  negotiated one is surfaced as `SocketInfo.alpn`; `Socket.upgraded` is exposed.
  `secureTransport: "starttls"` / `startTls()` and TLS on `listen` remain
  unsupported (DECISIONS D28).

- **URL component setters.** The `host` and `hostname` setters now fully comply
  with WHATWG specification. Setting a `hostname` with an invalid port ignores the
  input, while setting a `host` properly parses and applies both the domain and the
  port (with standard ports correctly dropping).
- **Benchmarks.** Added a new benchmark workload `url_setter` to specifically
  measure the overhead of modifying URL components across the JS/Rust boundary.

### Fixed

- **`esrun upgrade`.** Release archives nest the binary under a versioned
  directory (`esrun-<version>-<target>/esrun`), but the updater looked for it at
  the archive root, so every upgrade failed after the download. It now points at
  the correct in-archive path, and selects the archive by extension so the choice
  no longer depends on release-asset order.

### Changed

- **Fetch and HTTP performance.** Host operations for HTTP and network boundaries now return native V8 structures (`Value::Object` / `Value::Array`) instead of serializing to JSON strings. Eliminating the intermediate `JSON.parse` overhead drastically accelerates HTTP server throughput (by over 40% on hello-world workloads).
- **Automated benchmarks.** The repository's benchmark runner now automatically drives and integrates external web-framework load tests (like Hono over `oha`) directly into the documentation's dynamic datasets without manual static entries.

- **Release checksums.** Replaced the per-archive `.sha256` sidecars with a
  single `checksums.txt` per release. The sidecars shared the platform target
  string with their archive, making `esrun upgrade`'s asset selection ambiguous;
  the install scripts now verify against the combined file.

## [0.3.0] - 2026-06-16

### Added

- **`esrun types --install`.** Writes the `runtime:` TypeScript definitions into
  `node_modules/@opentf/esrun` (as a type package) and wires them into
  `tsconfig.json` (`typeRoots` + `types`) so editors and `tsc` resolve the
  `runtime:*` modules with no manual steps. Merges an existing config
  non-destructively, creates one if absent, and leaves JSONC configs untouched
  (printing the lines to add).

- **Standard error diagnostics.** Uncaught exceptions and unhandled promise
  rejections now report a JS **stack trace** with source position, printed as one
  coherent CLI error block with optional color (`NO_COLOR` honored).

- **Install script** can offer to add `esrun` to your `PATH`.

### Changed

- **`runtime:fs` small-file fast paths.** `read`/`write`/`stat`/`exists`/
  `readDir` on files under 64 KB run synchronously, skipping the async task
  hand-off; root-jail path resolution is fast-pathed — and re-canonicalized on
  every call, so a path that later becomes a symlink escape is always re-checked.

- **`runtime:fs` writes avoid redundant copies.** A `Uint8Array` is moved across
  the boundary instead of copied twice, and a string is encoded to UTF-8 once on
  the Rust side (no intermediate JS `TextEncoder` buffer). Large writes are
  measurably faster.

- **Benchmarks.** Added **LLRT** as a fifth runtime and reworked the harness for
  fair, contention-resistant numbers: runtimes run interleaved in randomized
  order, each cell is the **min** over repetitions (the contention-free floor)
  after a discarded warmup, noisy cells are flagged, and an opt-in `QUIET=1` mode
  pins the CPU and disables ASLR. Numbers regenerated.

- **`runtime:http` per-request cost trimmed further (~20.8 → ~18.2 µs CPU/req).**
  Two prelude refinements: a string `Response`/`Request` body is kept as a string
  and encoded lazily — the server hands it straight to `http_respond`, which
  encodes it Rust-side, dropping a per-request `utf8_encode` op crossing and an
  intermediate JS `Uint8Array`; and the trusted server `Request` builds its
  `Headers` object only when a handler reads `req.headers`. Behavior unchanged
  (`text()`/`arrayBuffer()`/`content-type` and header-reading handlers verified).
  Measured by **server CPU-time per request** (contention-immune; wall-clock
  req/s on a shared box is too noisy to compare). The residual gap to Bun/Deno
  (~12 µs) is the injectable-provider + driven-loop seam — a channel handoff and
  op/promise round-trip per request, by design.

## [0.2.0] - 2026-06-15

### Changed

- **`runtime:http` throughput — per-request cost cut ~30% (≈35k → ≈49k req/s,
  hello-world plaintext).** Four changes to the request path, all under the hood:
  the accept loop **batches** — one `http_next_request` crossing now drains many
  already-queued requests (`HttpServerProvider::next_request` → `next_requests(id,
  max)`, an embedder-visible trait change); request metadata crosses as a
  **structured array** instead of a per-request JSON string built in Rust and
  `JSON.parse`d in JS; the response body is read **synchronously** from the
  `Response` (no `await arrayBuffer()` round-trip) via an internal `_parts()`
  accessor; and a server `Request` reuses the **host-validated URL** instead of
  re-parsing it (internal `__serverRequest`, gated by a closure-private symbol so
  the public `Request` constructor's eager validation is unchanged). Measured
  with an external load generator (`oha`); see `bench/README.md`.

- **Driven loop now wakes on readiness, not on a fixed interval.** The standalone
  `Driver` injects a real `Waker` (`Runtime::set_async_waker` / `Engine::set_async_waker`)
  into the engine's async-op polling, and a newly-dispatched op wakes the loop
  immediately. Previously the loop re-polled pending async ops on a blind ~1ms
  interval, so each I/O op paid up to a full interval of latency. Now a ready op
  re-ticks at once (a 1ms fallback remains only for futures that register no
  waker). Sequential HTTP round-trip latency dropped from ~13 ms to ~0.14 ms per
  request; `fetch` and the `fs` workloads regained their proper latency, and
  `runtime:http` under concurrent load now outpaces Node. Embedder-visible API:
  `Engine::set_async_waker` (default no-op) and `Runtime::set_async_waker`.

### Added

- **Benchmarks** — added an **http** workload (each runtime's own HTTP server,
  2 000 requests in batches of 100 concurrent over loopback); numbers on the
  benchmarks page regenerated.

- **`runtime:http`** — an HTTP/1.1 server, the fifth `runtime:` standard module:
  `serve((request) => response)`. The handler takes a web `Request` and returns
  (or resolves to) a web `Response` — the same Fetch API objects `fetch` uses;
  a thrown error or non-`Response` return becomes a `500`. `serve(options?,
  handler)` returns a `Server` with `addr` (resolves to the bound address —
  `port: 0` picks an ephemeral one), `finished`, and `stop()`. Backed by a new
  injectable `HttpServerProvider` (vetted **hyper** 1.x, `SystemHttpServer`;
  each connection served on its own task, requests handed to the single-threaded
  isolate in batches) and gated on `Capability::NetListen` (like `runtime:net`
  `listen`). Request/response bodies are buffered; TLS is not supported yet. New
  `examples/modules/http.mjs` and `runtime-http.d.ts`.

- **`runtime:net`** — TCP sockets, the fourth `runtime:` standard module.
  `connect(address, options?)` follows the WinterTC Sockets API (returns a
  `Socket` synchronously; `.opened`/`.closed` promises, `.readable`/`.writable`
  web streams, closing the writable half-closes with FIN). `listen(options)`
  binds a server and yields inbound `Socket`s as an async-iterable `Listener`
  (`addr`/`accept()`/`close()`; `port: 0` picks an ephemeral port). Backed by a
  new injectable `NetProvider` (tokio `SystemNet`, spawned reader/writer tasks)
  and ops gated on `Capability::Net` (connect) / new `Capability::NetListen`
  (listen). TLS (`secureTransport`/`startTls`) is not supported yet. Also added
  `ReadableStream` async iteration (`values()` / `[Symbol.asyncIterator]`). New
  `examples/modules/net.mjs` and `runtime-net.d.ts`.

- **`esrun upgrade`** — self-update built into the CLI: finds the latest GitHub
  release for the platform, downloads + extracts it, and replaces the running
  binary in place (rustls TLS, via `self_update`). The Installation page's
  Upgrade step now uses it.

- **`@opentf/esrun-types`** — hand-written TypeScript definitions for the
  `runtime:` standard modules (`runtime:process`, `runtime:path`, `runtime:fs`,
  `runtime:net`, `runtime:http`),
  in [`types/`](types/), for editor completion and type-checking. Ambient
  `declare module` blocks; add via `tsconfig` `types` or a triple-slash
  reference. Validated with `tsc --strict`. Also emitted by **`esrun types`**
  (`esrun types > esrun.d.ts`, Deno-style) and shipped under `types/` in the
  release archive — the definitions are baked into the binary as a static
  string, so they add nothing to startup or runtime cost.
- **Benchmarks** — split the file-I/O workload into **read / write / append**
  and added a **glob scan** workload (Deno has no built-in runtime glob → n/a),
  all cross-runtime; numbers regenerated on the benchmarks page.

- **`runtime:fs`** — modern, Blob-based file I/O, the third `runtime:` standard
  module (SPEC §11, DECISIONS D25). `file(path)` is a lazy, Blob-like handle
  (`text`/`json`/`bytes`/`arrayBuffer`/`stream`/`exists`/`stat`/`write`/`delete`,
  plus `writable()` — a web-standard `WritableStream` for piped/incremental
  writes). `write(dest, body, { append })` takes any web body
  (string/Blob/ArrayBuffer/TypedArray/Response/ReadableStream/`file()`). Plus
  `readDir`, `stat`, `exists`, `mkdir`, `remove`, `rename`, and a `Glob`
  (`match` pure/sync, `scan` async over the jailed walk; `globset`/`walkdir`,
  `**`/`{a,b}` semantics). All operations are **async** (no sync variants, no
  callbacks). Backed by a new injectable `FileSystem` provider (tokio
  `SystemFileSystem`) and ops gated on new `Capability::FileRead` /
  `Capability::FileWrite`; every path is confined to the project **root jail**
  (D25 — `..`/symlink escapes rejected). New `examples/modules/fs.mjs`.

- **`runtime:path`** — modern, platform-aware path utilities, the second
  `runtime:` standard module (DECISIONS D26, SPEC §11). A pure-computation ES
  module that takes the host platform and `cwd()` from `runtime:process` (so it
  carries `Env`); separators and `resolve()` follow the real OS. Exports `sep`,
  `delimiter`, `isAbsolute`, `normalize`, `join`, `resolve`, `dirname`,
  `basename`, `extname`, `parse`, `relative`, and `file:` URL interop
  (`fromFileURL`/`toFileURL` — `dirname(fromFileURL(import.meta.url))` is the
  modern `__dirname`). One platform-correct surface: no `posix`/`win32` dual
  namespaces and no overloaded signatures. New `examples/modules/path.mjs`.

## [0.1.0] - 2026-06-14

### Project

- **API versioning starts at 0.1.0** (was `0.0.0`). Semver from here; the public
  Rust API and the `runtime:` standard-module namespace are the versioned
  contract. Locked v1 direction in DECISIONS **D24**: single repo serving both
  embedding and standalone use; **ESM-only, permanently** (no CommonJS interop);
  host capabilities exposed as async `runtime:` modules (`runtime:fs`,
  `runtime:net`, `runtime:http`, `runtime:process`) rather than globals;
  filesystem **root confinement by default** (CLI opt-out); Windows CI next,
  macOS after.

### Added

- **`runtime:` standard modules + `runtime:process`** — a built-in module scheme:
  `runtime:<name>` is served by the runtime itself (loader-independent, never
  touches the filesystem), with the capability check in the ops. First module
  `runtime:process` exposes `env` (mutable in-process snapshot), `args` (user
  args), `cwd()`, `platform` + `arch` (OS-native, e.g. `"linux"`/`"x86_64"`), and
  `exit(code = 0)` (halts + sets the process exit code) — gated on a new
  `Capability::Env`, backed by a new `Process` provider (`SystemProcess` reads
  the real process; embedders inject a controlled view). Aligned in spirit with
  the WinterTC CLI-API proposal (DECISIONS D26). New `examples/modules/process.mjs`.
- **ES modules** — `esrun` now runs every input as an ES module: static
  `import`/`export`, `import.meta.url`, and native top-level `await`. Imports
  resolve as **local files** (relative/absolute paths or `file:` URLs) through a
  new capability-checked `ModuleLoader` provider; `default-providers` ships
  `FsModuleLoader` (file-backed) and a deny-all default. The engine gained
  module compile/instantiate/evaluate behind an opaque `ModuleId` (no V8 type
  crosses the boundary), and `runtime` gained an async graph loader
  (`Runtime::load_module_source`) that walks + dedups the import graph before
  V8's synchronous instantiation, then settles top-level await on the driven
  loop. Loading an import requires `Capability::FileSystem`; a self-contained
  module runs without it. **Backward-incompatible:** inputs now run in module
  scope (strict mode, `this === undefined`), and the old async-IIFE wrapper for
  top-level await is gone (modules provide it natively). Import attributes /
  JSON modules and remote modules are not yet supported (DECISIONS D21). New
  `examples/modules/`.
- **Dynamic `import()`** — `import(specifier)` resolves with the module
  namespace after the imported module (and any top-level await in it) fully
  evaluates, and shares instances with static imports via a realm module map.
  The engine installs V8's host-import callback and settles the request once
  evaluation completes; `runtime` stores the loader and exposes an async
  `process_dynamic_imports()` drive step the `Driver` calls each iteration.
  Works for everything the static loader supports (local files + `node_modules`
  ESM packages). DECISIONS D23. New `examples/modules/dynamic.mjs`.
- **`node_modules` resolution (ES module packages)** — bare specifiers
  (`import x from "pkg"`, `"pkg/sub"`, `"@scope/pkg"`) resolve against an
  existing `node_modules` tree via the new `NodeModuleLoader`: walk
  `node_modules` upward, read `package.json` (`exports` string + `import`/
  `default` conditions + subpath patterns like `"./fn/*"`, or
  `module`/`main`/`index`), probe `.js`/`.mjs`/`.cjs`.
  **ES module packages only** — CommonJS packages and `node:` builtins are
  rejected with a clear message; nothing is installed (run `npm install`
  yourself). This narrows the no-npm non-goal (SPEC §125 amended; DECISIONS
  D22). `ModuleLoader::resolve` is now **async** (resolution does I/O); the
  strict file-only `FsModuleLoader` is kept for embedders wanting no
  `node_modules`. Adds `serde_json` to `default-providers` (already present
  transitively — no new crate).

### Performance

- **Prelude snapshot baked into `esrun`** — `build.rs` now builds the V8 startup
  snapshot at compile time and `include_bytes!`s it into the binary; the CLI
  restores it via `Runtime::with_snapshot` instead of compiling + evaluating the
  ~16 prelude files on every launch. Startup drops to ~6.6 ms (fastest of
  node/bun/deno/esrun on the bench box). Host-arch builds only — cross-compiling
  the CLI would need a target-run step (noted in `build.rs`).
- **Op-backed `atob`/`btoa`** — base64 transcoding moves from a pure-JS
  per-character concatenation into host `base64_encode`/`base64_decode` ops
  (`base64_ops.rs`); ~4.5× faster on the base64 workload (386 → 86 ms). Same
  semantics, including forgiving-base64 decode (with one recorded looseness:
  all trailing `=` are stripped).
- **URL ops return offsets, not JSON** — `url_parse`/`url_set` now return the
  canonical href plus 15 component offsets (`url::Position`s, UTF-16 indices)
  as one small JS array (new `Value::Array`); every `URL` getter is a lazy
  `href.slice(...)` and `.origin` is a separate lazy op. Replaces the 11-field
  JSON round-trip (~3× faster URL workload in `bench/`); same shape Node's Ada
  integration uses. Wire format documented in `url_ops.rs`/`url.js`.
- **Zero-copy op returns** — op results are consumed, not cloned: a returned
  `Value::Bytes` vec moves into the `ArrayBuffer` backing store (was: two extra
  copies per `TextEncoder.encode`), `utf8_encode` reuses the marshaled string's
  buffer, and `utf8_decode` converts valid UTF-8 in place. The JS→Rust crossing
  still copies (zero-copy there remains D3a/Phase 8).
- **Lazy HTTP client** — `ReqwestTransport` builds its reqwest client (TLS
  config, root store) on first `fetch` instead of at construction. Startup
  drops ~15 ms → ~8.5 ms (fastest of node/bun/esrun on the bench box); scripts
  that never fetch never pay for the client.
- **Sub-millisecond `performance.now()`** — new defaulted
  `Clock::monotonic_micros` (overridden by `SystemClock`); `performance.now()`
  now has µs precision instead of integer ms. Deterministic/test clocks are
  unaffected (default derives from `monotonic_ms`).
- **Release profile** — `lto = "thin"` + `codegen-units = 1` for the Rust-side
  hot paths (V8 is prebuilt; unaffected). `panic = "abort"` deliberately not
  set (D15 containment relies on unwinding).

### Fixed

- **`console.log` object inspection** — replaced the `JSON.stringify`-based
  formatter (which silently dropped function-valued properties, `undefined`,
  symbols, etc. — so an object/module-namespace full of functions printed as
  `{}`) with a recursive `util.inspect`-lite: functions as `[Function: name]` /
  `[class Name]`, arrays/objects/Map/Set/Error/RegExp/Date, null-prototype and
  module-namespace objects, nested quoting, a depth limit, and a circular guard.
  (A namespace import of a function-only package such as `moderndash` now prints
  its members instead of `{}`.)
- **`TextEncoder.encodeInto`** — `read`/`written` are now spec-correct under
  truncation: output is cut at a UTF-8 code-point boundary (never mid-sequence)
  and `read` counts only the UTF-16 code units actually encoded (was: always
  reported the full source length).

### Benchmark

- `bench/` reworked and broadened from 4 workloads to 15 plus a peak-RSS row.
  The `webapi` workload is split into `url` and `encoding` (separately
  attributable); new workloads add a pure-engine `json` baseline, large-document
  `jsonbig`, key-based `crypto` (HMAC + AES-GCM), `base64`, `structured`
  (`structuredClone`), `async` (microtask overhead), `timers`, `streams`,
  `fetch` (against a local server — the first workload to exercise the network
  provider seam), and `bigscript` (user-source parse cost). Deno is now detected
  (incl. `~/.deno/bin/deno`); workloads run an untimed JIT warmup and report the
  **median** of `WORKLOAD_RUNS` (default 5); `BENCH_JSON=1` emits machine-
  readable output and `WORKLOADS=...` runs a subset. Representative results
  refreshed across all four runtimes.

### Performance (earlier in this cycle)

- **Op-backed `TextEncoder`/`TextDecoder`** — UTF-8 transcoding now rides V8's
  native UTF-16↔UTF-8 conversion via `utf8_encode`/`utf8_decode` ops instead of
  a pure-JS code-point loop. ~47% faster on encode+decode; behaviour unchanged
  (fatal/BOM/replacement still correct). (Investigated structured marshaling for
  the URL path — returning a built JS object instead of JSON — and **reverted
  it**: per-property Rust→V8 object construction is slower than V8's native
  `JSON.parse`. Noted in `bench/README.md`.)
- **Lazy `URLSearchParams`** — `new URL()` no longer parses the query into a
  `URLSearchParams` eagerly; it's built on first `.searchParams` access. Cuts
  ~38% off URL construction for the common case that never reads `.searchParams`
  (no behaviour change; setters resync only a materialized instance). Measured
  in the cross-runtime benchmark (`bench/`).

### Tooling — standalone `esrun` CLI + crate rename

- **`esrun`** (`es-runtime-cli`) — a standalone binary that wires the default
  tokio providers and runs a JavaScript file or `-e <code>` snippet end-to-end
  (the §8 standalone embedding). Grants all capabilities (trusted-local-script
  mode). Inputs run as ES modules (see **Added** above), with native top-level
  `await`. **Single self-contained binary** — V8 is statically linked and the
  prelude is embedded; no asset directory. Example scripts under `examples/`;
  `cargo build-cli` builds it; `cargo install --path crates/runtime-cli` puts
  `esrun` on `PATH`.
- **Crate rename:** the flagship library crate `es-runtime-runtime` → **`es-runtime`**
  (import `es_runtime`); directory stays `crates/runtime`.

### Phase 9 (in progress) — Hardening: the safety spine

The resource-limit and FFI-safety guarantees (SPEC.md §4) that demonstrably stop
a runaway or heap-bomb script without harming the host. Fuzzing, sanitizer CI,
WPT conformance, and byte/BYOB streams remain for later Phase 9 passes.

#### Added

- **Execution watchdog** — `engine` exposes a thread-safe `InterruptHandle`
  (`terminate`/`is_terminating`; names no V8 type, so it stays within the engine
  boundary D3) and `Engine::interrupt_handle()`. `eval` detects a
  watchdog/heap termination and returns `Error::Terminated { reason }` rather
  than hanging; the engine recovers (the terminating state is cleared).
- **Near-heap-limit guard** — terminates execution and grants unwind headroom,
  so a heap bomb surfaces as `Terminated("heap limit exceeded")` instead of an
  OOM crash.
- **Bounded pending-ops** — `OpState` enforces `Limits::max_pending_ops`; the
  over-limit async dispatch throws a `RangeError`.
- **Panic-across-FFI containment (resolves D15)** — the V8-invoked callbacks
  (`op_dispatch`, `timer_set`, `timer_clear`, `promise_reject_callback`) run
  inside `catch_unwind`; a Rust panic in a host op handler or in marshaling is
  contained as a JS exception, never an unwind across V8's C++ frames (assumes
  `panic = "unwind"`).
- **Stack guard** — documented + tested: V8's native guard turns unbounded
  recursion into a catchable `RangeError`.
- **`esrun -t/--timeout <ms>`** — a watchdog thread terminates the engine after
  the deadline (cross-thread V8 termination stops even a synchronous infinite
  loop), with a tokio-timeout backstop for async-callback runaways. `Runtime`
  exposes `interrupt_handle()`.

- **Internal security review + docs finalization** — `docs/SECURITY-REVIEW.md`:
  a consolidated threat model, trust boundaries, attack-surface→defense table,
  and a residual-risk register (fuzzing/external-review pending, `rsa` advisory,
  SES deferral, `panic=abort` caveat, watchdog scope). Finalized SPEC §8
  definition-of-done status, refreshed ARCHITECTURE §7/§9 (intrinsic integrity,
  snapshot done / zero-copy deferred), and cross-linked from `SECURITY.md`.
- **Intrinsic-integrity audit** (§4) — confirmed + documented that the security
  boundary is in Rust: the op table and capability set live in `OpState`, so
  guest JS tampering (prototype pollution, global reassignment, forging
  `__ops`) can't escalate privilege or dispatch an ungated op. Added `harden.js`
  (last prelude fragment) as defense-in-depth: locks the `globalThis.__ops`
  binding (object stays extensible for op registration) and freezes `console`.
  3 tamper-resistance tests. SES-style primordial freezing is deliberately
  deferred to the embedder/Layer B (SECURITY.md), not baked into Layer A.
- **Byte/BYOB streams** (§2.8, closes the §7 deferral) — `ReadableStream`
  `type: "bytes"` + `ReadableByteStreamController`, `ReadableStreamBYOBReader`,
  `ReadableStreamBYOBRequest`, `autoAllocateChunkSize`, the pull-into queue, and
  `byobRequest.respond`/`respondWithNewView`, hand-written to the WHATWG abstract
  operations (DECISIONS D19). Copy-based: enqueued chunks are copied into
  controller-owned buffers and BYOB views filled in place — no ArrayBuffer
  transfer/detach (single-threaded; zero-copy is the D3a follow-up). 5 new
  conformance assertions (now 62/62).
- **Conformance suite + pass-rate tracking** (§5/§8) — a curated in-repo set of
  spec-behaviour assertions (`crates/runtime/conformance/*.js`: encoding, base64,
  URL, structuredClone, events, abort, crypto, streams, performance) run by the
  `conformance_suite_passes` test, which is a CI gate. Zero-failure + a
  non-regressing count are enforced; the snapshot (currently **57/57**) is
  recorded in `conformance/RESULTS.md`. An in-JS harness provides
  `test`/`assert*` (sync + async).

#### Tests

- Watchdog stops a `while(true){}` from another thread (engine recovers after);
  a heap bomb is terminated cleanly; a panicking op surfaces as a catchable JS
  `Error`; the pending-op bound rejects the over-limit call; deep recursion is a
  typed error. Verified end-to-end via `esrun -t`.

### Phase 8 — Startup snapshot + perf

Bakes the prelude and op shells into a V8 startup snapshot (SPEC.md §6.8,
DECISIONS.md D8), so constructing a runtime can skip compiling *and* running the
prelude.

#### Added

- **`V8Engine::build_snapshot(configure)`** — runs op registration + the prelude
  into a snapshot-creator isolate and serializes the heap — and
  **`V8Engine::with_snapshot_baked_ops`** to restore it. The native callbacks
  (`op_dispatch`, `timer_set`, `timer_clear`) are registered as one canonical
  **external-reference list** supplied at both build and restore (matched by
  index, so ASLR-safe across processes).
- **`Runtime::build_snapshot(providers)`** and **`Runtime::with_snapshot(blob,
  providers)`**: the restore path rebinds only the Rust op handlers (the JS
  `__ops.<name>` shells and the prelude are baked) in the same order
  `build_snapshot` used, and skips prelude evaluation entirely.
- A lightweight **`bench` example** (`default-providers`, std-only — no bench
  framework) measuring fresh vs snapshot startup and op-dispatch throughput.
  Indicative: ~**2.3× faster** runtime startup from a snapshot.

#### Changed / audited

- **Zero-copy `ArrayBuffer` transfer audited and deferred** (D3a Phase 8): the
  `Value::Bytes` in-copy (`copy_contents`) is unsafe to elide while async ops
  outlive the call scope; the out-copy (`bytes.to_vec()`) is a low-risk
  follow-up. Both kept as copies for now — correct and bounded by body size.
- Only the JS heap is serialized into the snapshot (context, `__ops.<name>`
  shells with their op-ids, prelude state); Rust handler closures are not.

### Phase 7b — WebCrypto (AES block modes, key derivation, elliptic curve, RSA)

Completes `crypto.subtle` (SPEC.md §6.7 / §2.10): the remaining symmetric
ciphers, the key-derivation functions, elliptic-curve ECDSA/ECDH, and RSA — all
RustCrypto (DECISIONS.md D9).

#### Added

- **AES-CBC** (`encrypt`/`decrypt`, PKCS#7 padding; 128/192/256-bit keys) and
  **AES-CTR** (`encrypt`/`decrypt`; 128/192/256-bit keys; 32/64/128-bit counter
  widths) on `crypto.subtle`, plus `generateKey`/`importKey` for both. One CTR
  op backs encrypt and decrypt (the mode is symmetric).
- **`deriveBits`/`deriveKey`** via **HKDF** (SHA-1/256/384/512) and **PBKDF2**
  (HMAC-SHA-1/256/384/512). KDF base keys import as non-extractable `raw` keys;
  `deriveKey` targets AES-* and HMAC derived keys.
- New ops `subtle_aes_cbc_encrypt`/`_decrypt`, `subtle_aes_ctr`, `subtle_hkdf`,
  and `subtle_pbkdf2`, backed by the `aes`/`cbc`/`ctr` and `hkdf`/`pbkdf2`
  RustCrypto crates. `aes`/`cbc`/`ctr` are pinned to the `cipher` 0.4 generation
  so they reuse the same `aes` 0.8 that `aes-gcm` already pulls (no duplicate
  `aes`; `aes-gcm` 0.11, which would unify onto `cipher` 0.5, is still an rc);
  `hkdf`/`pbkdf2` 0.13 reuse the existing `hmac` 0.13 + `sha2`.
- Tests add NIST SP 800-38A vectors (CBC F.2.1, CTR F.5.1), RFC 5869 (HKDF) and
  RFC 6070 (PBKDF2) known-answer vectors, round-trips, and a PBKDF2→AES-GCM
  `deriveKey` end-to-end.
- **ECDSA** (sign/verify) and **ECDH** (`deriveBits`/`deriveKey`) over **P-256,
  P-384, P-521** on `crypto.subtle`, with `generateKey` (key pairs) and
  `importKey`/`exportKey` for **all four formats** (`raw`/`spki`/`pkcs8`/`jwk`).
  ECDSA honours an arbitrary `algorithm.hash` (SHA-1/256/384/512). New
  `ec_ops` module + ops (`ec_generate_pkcs8`, `ec_public_point`,
  `ec_private_scalar`, `ec_import_pkcs8`, `ec_pkcs8_from_scalar`,
  `ec_import_spki`, `ec_export_spki`, `ecdsa_sign`, `ecdsa_verify`,
  `ecdh_derive`), backed by `p256`/`p384`/`p521`.
- EC keys cross the op boundary as PKCS#8 (private) / SEC1 points (public); JWK
  is assembled in JS from the exposed coordinates/scalar. **ECDSA signing draws
  its nonce from the `Entropy` provider** (hedged `RandomizedPrehashSigner`),
  never ambient `OsRng` — notable for P-521, whose deterministic path otherwise
  reaches for `OsRng`.
- The EC crates sit on the older `elliptic-curve` 0.13 / `digest` 0.10
  generation (0.14 is pre-release), so they bring **duplicate `digest` 0.10,
  `sha2` 0.10, and `hkdf` 0.12** — warn-level under `deny.toml`, accepted per
  DECISIONS.md D9.
- Tests cover ECDSA P-256 sign/verify (+ tamper) and P-521/SHA-512, a P-384
  export→import round-trip across **all four formats**, ECDH shared-secret
  agreement, and an ECDH→AES-GCM `deriveKey` between two parties.
- **RSA** — **RSASSA-PKCS1-v1_5** and **RSA-PSS** (sign/verify) and **RSA-OAEP**
  (encrypt/decrypt) on `crypto.subtle`, with `generateKey` (key pairs) and
  `importKey`/`exportKey` for **spki/pkcs8/jwk** (private JWK incl. the CRT
  params `d`/`p`/`q`/`dp`/`dq`/`qi`). Arbitrary `algorithm.hash`
  (SHA-1/256/384/512). New `rsa_ops` module + ops backed by the `rsa` crate;
  JWK components cross the boundary via a small length-prefixed framing.
- All RSA randomness (key gen, PSS salt, PKCS#1 blinding, OAEP padding) routes
  through the **Entropy provider**, never ambient `OsRng`. `rsa`/`num-bigint-dig`
  are built at `opt-level = 3` in the dev profile so test-suite key generation
  stays fast (~1.4 s vs ~33 s).
- **Accepted security gap:** the `rsa` crate carries **RUSTSEC-2023-0071**
  (Marvin timing sidechannel, medium, no fix available). Maintainer-accepted
  with rationale — RSA private-key ops are host-side, and the alternatives
  (aws-lc-rs: ambient RNG + C backend; openssl-rs: system dep) cost more than
  they buy. Listed explicitly in `deny.toml` + `.cargo/audit.toml`; tracked on
  the new **`SECURITY.md`** revisit list. RSA-OAEP labels are UTF-8 only (an
  `rsa` 0.9 API limitation).
- New `SECURITY.md` records the project's supply-chain posture and the accepted
  advisory gaps (RSA Marvin, `paste` unmaintained).
- Tests: one 2048-bit key reused across PKCS1-v1_5 + PSS sign/verify, OAEP
  round-trip (with and without a label), and SPKI/PKCS8/JWK export→import with
  cross-verification.

### Phase 7 — WebCrypto (first tranche)

`crypto` (SPEC.md §6.7 / §2.10), backed by vetted RustCrypto primitives
(DECISIONS.md D9). Resolves the open D9 crypto-backend decision.

#### Added

- **`crypto.getRandomValues`** (fills an integer TypedArray in place) and
  **`crypto.randomUUID`** (v4), drawing from the `Entropy` provider — now wired
  into `HostProviders` (the D16-anticipated point).
- **`crypto.subtle`** (first tranche): `digest` (SHA-1/256/384/512), **HMAC**
  (`generateKey`/`importKey`/`exportKey`/`sign`/constant-time `verify`), and
  **AES-GCM** (`generateKey`/`importKey`/`exportKey`/`encrypt`/`decrypt`, tag
  mismatch → `OperationError`). Plus the `CryptoKey` class.
- Crypto runs in synchronous `runtime` ops (RustCrypto: `sha1`, `sha2`, `hmac`,
  `aes-gcm`); the prelude `subtle` wraps each in a Promise.
- Tests use known-answer vectors (SHA-256("abc")), HMAC sign/verify (incl.
  tamper), and AES-GCM round-trip + tamper rejection.

#### Decisions

- **D9 locked: RustCrypto** (breadth + portability). ECDSA/ECDH and RSA are
  staged for **Phase 7b** (SPEC §7). The TLS backend (D20) is independent.

### Phase 6 — Fetch family

`fetch` and its surrounding types (SPEC.md §6.6 / §2.9), networking routed
exclusively through a new `NetTransport` provider; response bodies stream via the
Phase 5 streams.

#### Added

- **Engine `Value::Bytes`** — the marshaler now converts `Uint8Array`/typed-array
  views ↔ `Vec<u8>` (copying), so byte bodies can cross the op boundary. True
  zero-copy `ArrayBuffer` transfer remains Phase 8 (DECISIONS.md D3a).
- **`NetTransport` provider** (`providers`) — outbound HTTP for `fetch`:
  `HttpRequest` (buffered body) → `HttpResponse` (metadata + a streamed
  `ByteStream` body, via `futures-core`). Capability-gated on `Capability::Net`.
- **default-providers** — `ReqwestTransport` (reqwest + rustls TLS, no OpenSSL;
  HTTP/1.1 + HTTP/2; streamed response bodies) and a deterministic
  `MockTransport`/`MockResponse` (testing) so fetch is tested without network.
- **runtime fetch** — capability-gated `fetch` async op + a `fetch_body_read`
  op that streams the response body into a JS `ReadableStream`. `HostProviders`
  gains the net provider.
- **Prelude**: `Headers` (case-insensitive, combining), the `Body` mixin
  (`arrayBuffer`/`text`/`json`/`blob`/`bytes`/`body` stream), `Request`,
  `Response` (+ `Response.json`/`error`), and `fetch`; `Blob`, `File`, and
  `FormData` (multipart encoding).
- New dependencies: `reqwest` (rustls), `futures-core`/`futures-util`; `url`
  unchanged. `deny.toml` allows `CDLA-Permissive-2.0` (the rustls root-cert
  bundle).

#### Decisions

- **D20** locked: after weighing a from-scratch HTTP client, use a **vetted HTTP
  crate** (reqwest + rustls) for the default `NetTransport` — HTTP/1.1 framing
  and TLS are security-sensitive, and **TLS may not be hand-rolled** (§7/D9).
  Confined to `default-providers`. Streaming model: **buffered request body,
  streamed response** for Phase 6; streaming request bodies are a follow-up
  (SPEC §7).

### Phase 5 — Streams

The Streams surface (SPEC.md §6.5 / §2.8) — the largest correctness item —
hand-written to the WHATWG abstract operations (DECISIONS.md D19), pure JS in the
prelude.

#### Added

- **`ReadableStream`** (default) + `ReadableStreamDefaultController` +
  `ReadableStreamDefaultReader`: enqueue/read/close/error/cancel, start/pull/cancel
  algorithms, `desiredSize` backpressure, `tee`.
- **`WritableStream`** + controller + writer: write/close/abort with the full
  erroring/abort state machine, `ready`/`closed` promises, backpressure.
- **`TransformStream`** + controller: transform/flush with backpressure linking
  the writable and readable sides.
- **`pipeTo`** (with `preventClose`/`preventAbort`/`preventCancel` + `AbortSignal`)
  and **`pipeThrough`**.
- **`CountQueuingStrategy`**, **`ByteLengthQueuingStrategy`**.
- **`TextEncoderStream`** / **`TextDecoderStream`** (deferred from Phase 4) on
  `TransformStream`, handling surrogate pairs / multi-byte UTF-8 split across
  chunk boundaries.
- A test harness (`eval_async`) that drives async JS to completion via the tick
  microtask loop.

#### Decisions

- **D19** locked (maintainer sign-off): Streams are **hand-written to spec**
  (fits the from-scratch ethos, D2) and **default-first** — byte/BYOB streams
  (`ReadableByteStreamController`, BYOB readers) are deferred to a follow-up
  (SPEC §7). Conformance tracked vs WPT (D13).

### Phase 4 — Core web primitives

The WinterTC pure-JS surface (SPEC.md §6.4), shipped as a JS prelude over the op
system, with world-touching parts as host ops.

#### Added

- **Prelude harness** — `runtime` now installs host ops and evaluates a JS
  prelude at construction (`Runtime::new` takes [`HostProviders`] and returns
  `Result`). Per D8 the prelude is snapshot-baked in Phase 8; evaluated at
  startup until then.
- **Console** as an injectable output sink (DECISIONS.md D17): a `Console`
  provider trait (guest output, not telemetry — boundable/attributable per §7),
  with `TracingConsole` (default → `tracing`), `NullConsole` (deniable), and
  `CapturingConsole` (tests). `console.*` formats args and routes through it;
  group/table are minimal.
- **performance** — `performance.now()` / `timeOrigin`, backed by the `Clock`
  provider (the D16 point where `runtime` gains its `providers` dependency).
- **Globals** — `queueMicrotask`, `reportError`, `structuredClone` (deep clone of
  the standard cloneable types incl. cycles; `DataCloneError` otherwise), and the
  `self` alias.
- **DOMException** — a real JS class in the prelude (closes the JS-class half of
  the D3a note), used by atob/btoa, structuredClone, and Abort.
- **Encoding** — `TextEncoder`/`TextDecoder` (UTF-8, pure JS) and `atob`/`btoa`.
- **URL family** — `URL` + `URLSearchParams`, parsing/serialization via the
  servo `url` crate behind sync ops (DECISIONS.md D18), with `search`/`searchParams`
  kept in sync.
- **Events** — `Event`, `CustomEvent`, `EventTarget` (flat dispatch: once,
  passive, signal, capture flag, `preventDefault`).
- **Abort** — `AbortController`, `AbortSignal` incl. `AbortSignal.abort`,
  `AbortSignal.timeout` (timer-driven), and `AbortSignal.any`.
- New dependency: `url`, in `runtime`.

#### Decisions

- **D17** (Console = injectable output-sink provider; default forwards to
  tracing) and **D18** (URL via the `url` crate) locked. Deferrals (SPEC §7):
  `TextEncoderStream`/`TextDecoderStream` → Phase 5 (need Streams)
  → later; full WHATWG-URL conformance gaps tracked vs WPT.

### Phase 3 — Provider traits + default tokio providers

The I/O integration seam (SPEC.md §6.3): provider traits, reference tokio-backed
implementations, deterministic test providers, and a standalone driver.

#### Added

- **`es-runtime-providers` crate** — trait definitions only, no impls, no
  `unsafe` (ARCHITECTURE.md §6, DECISIONS.md D5): `Clock` (monotonic + wall ms),
  `Entropy` (fill CSPRNG bytes), `Timers` (`sleep` future), `TaskSpawner`
  (offload blocking work). `ProviderError` maps to a JS exception via
  `IntoException`. (`NetTransport`/`FileSystem` arrive with their consuming APIs.)
- **`es-runtime-default-providers` crate** — the **only** crate owning a real
  loop/clock/entropy:
  - Production impls: `SystemClock` (std `Instant`/`SystemTime`), `OsEntropy`
    (`getrandom`), `TokioTimers` (tokio timer wheel), `TokioTaskSpawner` (tokio
    blocking pool).
  - `Driver` — runs a `Runtime` to quiescence on tokio: reads the `Clock` for
    each tick's time, parks on `Timers` between ticks, accumulates unhandled
    rejections. This is the concrete loop `runtime` deliberately does not own
    (D4); Layer B swaps it for its scheduler.
  - `testing` module — deterministic providers (`ManualClock`, `ManualTimers`
    that advance a linked clock, seeded non-crypto `SeededEntropy`,
    `InlineTaskSpawner`) for reproducible runs (D5). The driver integration test
    runs an async op + a timer to completion with zero real waiting.
- New dependencies: `tokio` (rt + time) and `getrandom`, confined to
  `default-providers`.

#### Decisions

- **Providers + driver only** (maintainer sign-off): Phase 3 does **not** change
  `runtime`'s public API. `runtime` keeps `tick(now_ms)` and gains a `providers`
  dependency only when a provider-backed web API lands (`performance.now` →
  Phase 4, `getRandomValues` → Phase 7). The `Driver` supplies tick time from the
  `Clock`. **D9 (crypto.subtle backend) remains open** — `getrandom` is raw OS
  entropy, not the algorithm backend.

### Phase 2 — Op system + driven loop

The JS↔Rust op bridge and the embedder-driven event loop (SPEC.md §6.2): sync +
async ops, promise resolution, a microtask checkpoint, the tick/poll API, and
timer plumbing.

#### Added

- **Engine abstraction trait.** Extracted `engine::Engine` (object-safe, names no
  V8 type) from the concrete type, now `engine::V8Engine` (DECISIONS.md D3). The
  trait is the surface `runtime` depends on — a second engine could be slotted in
  without editing `runtime`.
- **`es-runtime-runtime` crate** — the driven runtime, built on the engine trait,
  with **zero direct `v8` dependency** and no `unsafe`. Holds a `Box<dyn Engine>`,
  the op wiring, and the timer schedule.
  - `Runtime::tick(now_ms) -> TickStatus` advances one step in order — due
    **timers → ready async ops → microtask checkpoint → unhandled rejections** —
    and reports work remaining + the next deadline so the embedder can park. No
    loop or thread is owned (DECISIONS.md D4).
  - `Runtime::register_op`, `set_capabilities`, `eval`, `has_pending_work`.
- **Op system** (`engine::op`) — a single non-capturing dispatch callback keyed by
  op id, op table in an isolate slot via `Rc<RefCell<_>>`:
  - Sync and async ops; arguments marshaled and **validated as untrusted**;
    **capability-check-first** dispatch (denied → clean JS exception, never a
    partial effect — ARCHITECTURE.md §4, D7). Ops exposed as `globalThis.__ops.<name>`.
  - Async ops return a real `Promise`; std-only **poll-on-tick** (no reactor,
    `Waker::noop`) settles them, then the microtask checkpoint runs reactions.
  - Errors carry their JS exception class via `OpError`/`IntoException`.
- **Timers** — `setTimeout`/`setInterval`/`clearTimeout`/`clearInterval` builtins;
  the engine holds the JS callbacks, the runtime owns the deadline-ordered
  schedule. Time is embedder-supplied per tick (the `Clock`/`Timers` providers
  become that source in Phase 3).
- **Unhandled-rejection tracking** via the promise-reject callback; surfaced in
  `TickStatus.unhandled_rejections`.
- Explicit microtask policy so reactions run only at the checkpoint, never
  implicitly mid-eval.

#### Decisions

- `runtime` introduced now (Phase 2) rather than Phase 4, and the engine trait
  extracted now — both per maintainer sign-off. New D3a leak notes: DOMException
  is not yet a real JS class (surfaced as `Error` with a name-prefixed message);
  async readiness is observed only on `tick`; timer JS callbacks stay in `engine`.

### Phase 1 — Foundation

Workspace, error model, observability, CI, and a V8 engine that runs `1 + 1`
end-to-end with snapshot scaffolding (SPEC.md §6.1).

#### Added

- **Cargo workspace** (`resolver = "3"`, edition 2024, MSRV 1.95) with the
  dependency direction from ARCHITECTURE.md §2 enforced by the crate graph
  (DECISIONS.md D11). Phase 1 introduces the first two crates; the rest land in
  their own phases.
- **`es-runtime-common`** — cross-cutting primitives, no I/O, no `unsafe`
  (`#![forbid(unsafe_code)]`):
  - Error model (DECISIONS.md D12): `ExceptionClass` JS-exception taxonomy, the
    `IntoException` trait each layer implements, the `common`-layer `Error`, and
    a `Result` alias.
  - `CapabilitySet` / `Capability` — deny-by-default capability tokens
    (DECISIONS.md D7); the empty set is the default.
  - `Limits` — per-isolate resource ceilings (heap, stack depth, pending ops)
    with validation and builder setters.
  - `telemetry::init_tracing` — idempotent `tracing` subscriber install
    (ARCHITECTURE.md §8).
- **`es-runtime-engine`** — the only crate using the `v8` crate (DECISIONS.md
  D2/D3):
  - One-time V8 platform init; `Engine` owning an isolate + a persistent
    context.
  - `Engine::eval` compiles and runs source under a `TryCatch`, marshaling JS
    primitives to `Value` and mapping failures to typed `Compile` / `Execution`
    errors — no panic crosses the boundary.
  - `snapshot::build` / `Engine::with_snapshot` — startup-snapshot build/load
    scaffolding (DECISIONS.md D8), proven by a prelude-state round-trip test.
  - The isolate heap ceiling from `Limits` is installed on creation.
- **CI** (`.github/workflows/ci.yml`) — all gates from SPEC.md §5: `fmt`,
  `clippy -D warnings`, `test`, `cargo-deny`, `cargo-audit`, and an MSRV (1.95)
  build.
- **Supply-chain config** — `deny.toml` with an Apache-2.0-compatible permissive
  license allowlist; `rust-toolchain.toml`, `rustfmt.toml`. One documented
  advisory ignore: `RUSTSEC-2024-0436` (`paste` unmaintained — informational
  only, reaches us transitively through `v8`, no fix available).

#### Decisions

- **D10 — License: Apache-2.0** locked (superseding the earlier AGPL-3.0 lean),
  matching the `LICENSE`/`NOTICE` already in the repo.
- **D3a** leak points recorded for the engine boundary (see DECISIONS.md):
  uncaught-exception JS class not yet preserved; primitive-only value marshaling;
  snapshot-creation concurrency constraint.

### Next

- **Phase 7b** — the rest of `crypto.subtle`: AES-CBC/CTR, ECDSA/ECDH (P-256/384/521),
  RSA (PKCS1/PSS/OAEP), HKDF/PBKDF2.
- **Phase 8** — bake the prelude into a V8 startup snapshot (D8); zero-copy
  ArrayBuffer audit; benchmarks.
- **Phase 9** — hardening: heap/CPU/stack limits, the watchdog, panic-across-FFI
  containment (D15), byte/BYOB streams, fuzzing, WPT conformance run.
