# Changelog for `esdev`

All notable changes to **`esdev`**, the ES-Runtime's local development binary,
are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

`esdev` is versioned **separately from `esrun`** and from the Rust crates: it is
a tool rather than a contract, and its command line moves at its own pace. The
runtime it runs your program on is `esrun`'s — same prelude, same snapshot, same
providers, same capability enforcement. So what changes here is everything
*around* a run, plus the development-only `runtime:` modules `esdev` adds on top
of the runtime's own; the standard library itself is `esrun`'s and changes in the
root [CHANGELOG.md](../../CHANGELOG.md).

Two things differ deliberately, and only these two. The **default grant**:
`esdev` starts from every capability and `esrun` from none (DECISIONS D65), so
that an inner loop needs no flags and a deployment states what it may reach. And
the **extra modules**: `runtime:build`, `runtime:test` and `runtime:watch` exist
only in this binary, so code that imports one does not run under `esrun` — which
is the point, since none of the three has any business in a deployment.

`esdev` is **not a deployment target**: ship the artifact and run it under
`esrun`, which has no development surface to attack.

## [Unreleased]

## [0.6.0] - 2026-09-01

### Added

- **`esdev test --setup=<path>`, `--timeout=<ms>` and `--reporter=json`**, each
  also an `esdev.json` key under `"test"` (with `jobs`). A flag beats the file.
  See the root [CHANGELOG.md](../../CHANGELOG.md).

- **`esdev.json` describes a library** — `"lib"`, `"format"`, `"types"` and
  `"dts-bundle"` as target keys, so `esdev build` publishes a package with its
  `assets` (the README and LICENSE) copied. A `--lib` build now copies them too;
  it never did.

- **Imports resolve the way `esdev build` resolves them.** `./util` finds
  `util.ts`, a directory finds its `index.*`, and `./util.js` finds the
  `util.ts` TypeScript tells you to spell that way.

  The two halves of this binary disagreed: `esdev build src/app.ts` bundled a
  source tree written for a build step without complaint, while `esdev
  src/app.ts` refused to run the same tree — and most published TypeScript is
  written that way. `esrun` is unchanged and still resolves only what the module
  spec says, so ship a build rather than the source. A miss reports the
  specifier the file wrote, not the last spelling tried.

- **`esdev test` discovers `*.spec.*` as well as `*.test.*`.** Both conventions
  are everywhere, and a runner that knows only one silently runs no tests in
  half the projects it is pointed at — which looks exactly like a suite that
  passes.

- **`runtime:test` gains `expect`, `mock` and `clock`.** See the root
  [CHANGELOG.md](../../CHANGELOG.md); the module is `esdev`'s, but the API is
  documented with the rest of the standard modules.

## [0.5.0] - 2026-08-19

### Added

- **`esdev.json` carries `plugins`.** A project that compiles something this
  toolchain does not know about — `.mdx`, Tailwind, another framework's
  components — could only be built by a *program* that called `build()` itself.
  `esdev build` and `esdev start` had nowhere to put a plugin, so a framework
  could not be a project's build; it had to be its own.

  ```json
  {
    "plugins": [
      "./plugins/mdx.js",
      { "module": "@otfw/compiler", "options": { "jsx": "automatic" } }
    ],
    "targets": {
      "server": { "entry": "src/server.ts", "out": "dist/server.js" },
      "browser": { "entry": "src/client.tsx", "outdir": "dist/client",
                   "platform": "browser", "plugins": ["./plugins/only-web.js"] }
    }
  }
  ```

  An entry is the module to import — a path in this project, or a package. The
  project's plugins apply to every target; a target's `plugins` are **added** to
  them rather than replacing them, so a project that compiles `.mdx` compiles it
  for the server bundle and the browser one. `export` names which export the
  plugin is, and `options` is what to call it with when that export is a
  factory.

  `options` is the one thing JSON cannot express on its own: `mdx({ … })` is a
  function application. So the file carries the argument and esdev makes the
  call — which is the whole of the difference from an executable config, and
  what keeps this file readable as *data*. `permissions` is still decidable
  without running anything, which was the reason the config was JSON to begin
  with.

  They load into an isolate of their own, started on the first build that needs
  one and held for the run. `esdev start` rebuilds on every save, and
  re-evaluating a plugin's module — and whatever it initialises: a compiler, a
  template cache, a Tailwind context — per keystroke would be a startup cost
  paid forty times a minute, and a plugin holding state across builds (every
  incremental compiler does) could not exist at all. The plugins run under
  `esdev`'s own grant, in the project directory, with the same `runtime:`
  namespace any other program gets.

  Their passes go into **one list with this toolchain's own**, so `order: "pre"`
  means before `esdev:css-modules` and not merely before the other plugins.

- **`runtime:build`: a `bundle` hook — what the build produced.** The sixth
  hook, and the one a plugin needed to work with the *result* of a build rather
  than only its inputs. It runs once, after the graph has been split into chunks
  and before any of it is written, with one entry per chunk (`fileName`, `name`,
  `isEntry`, `isDynamicEntry`, `facadeModuleId`, `moduleIds`, `imports`,
  `dynamicImports`) or asset.

  Route-level `modulepreload` is the case it exists for: which chunk an entry
  became, what went into it and what it imports do not exist until the split,
  and the split happens *after* `end`. That is why `end` is handed `null` and
  not the bundle — it fires when the module graph is finished, when there are no
  chunks yet.

  Read-only, and carrying no `code`. Rollup lets a plugin rewrite chunks in the
  equivalent hook, which is how one plugin comes to invalidate the source maps
  of every plugin after it; and the bytes are what `generate()` already returns,
  so copying every chunk into the isolate on each rebuild would be a cost paid
  by a hook that only wanted the shape.

- **A chunk says which module it is.** `facadeModuleId` — the entry a chunk was
  built for, or the module behind a dynamic import, and `null` for a shared
  chunk. Rollup and rolldown both report it and this did not, so code written
  against it read `undefined` and fell through to `find((c) => c.isEntry)` —
  which picks the wrong chunk the moment a build emits a worker, since an
  emitted worker chunk is an entry too.

- **A plugin can declare what the *compiler* has to do for it.** Not a hook: a
  hook is handed a module's source and hands source back, and that is enough for
  almost everything — but the JSX pass runs inside the bundler, where a plugin
  cannot reach.

  ```js
  export default {
    name: "react-refresh",
    jsx: { refresh: true },
    transform: { filter: { id: /\.[jt]sx$/ }, handler },
  };
  ```

  `jsx.refresh` asks for a registration per component and a signature per
  hook-using function — what a component-refresh scheme matches components up by
  so an edit re-renders in place instead of remounting. Finding those needs the
  syntax tree the compiler already has; the per-module half the plugin writes
  itself. Honoured only in a hot dev build of a target that named a `refresh`
  scheme, which is a safety property rather than a policy: the calls inserted
  reach globals that only a hot loop installs.

- **`runtime:test`: `beforeAll`, `afterAll`, `beforeEach`, `afterEach`.** Each
  may be registered more than once and all of them run, in registration order —
  a helper module and the test file both have a right to a `beforeEach`.
  `afterEach` runs after a test that failed, because it is cleanup; a
  `beforeAll` that throws fails every test in the file rather than letting them
  run against a fixture that was never built.

  ```js
  let db;
  beforeEach(async () => { db = await open(":memory:"); });
  afterEach(() => db.close());
  ```

- **A build error says where it happened.** `generate()` and `write()` throw a
  `BuildError` whose `errors` is **every** diagnostic of the batch, each with
  `{ message, id, plugin, kind, line, column, frame }` — line 1-based, column
  0-based in UTF-16 code units, which is what an editor counts in, and `frame`
  the offending line with the span underlined.

  ```js
  catch (err) {
    for (const e of err.errors) {
      overlay.show(`${e.id}:${e.line}:${e.column}`, e.frame ?? e.message);
    }
  }
  ```

  A failure used to be a string — the module id, a colon and *"Unexpected
  token"* — which is enough to open the right file and nothing more: an editor
  overlay could name the file and then had to stop. The bundler computed the
  rest all along for its own terminal output. `esdev build` prints the frame
  too, in place of the one-line summary.

- **`ctx.type` on `transform`** — what the module *is now*, which is not always
  what its extension says, since a pass ordered `pre` may already have changed
  it. It is how a pass declines work somebody else has done.

### Changed

- **Hot reloading is generic, and React moved out into a plugin.** **Breaking**
  for a project that set `"refresh": "react"` and did not create its
  `esdev.json` with a current `esdev create`.

  `esdev` implemented one framework's refresh scheme and knew its name, so
  `"react"` was both the only value `refresh` would take and the only thing that
  could implement it. Every other framework took a full page reload on each
  edit — not because the mechanism was missing (`import.meta.hot` is the same
  for everyone) but because there was nothing generic to hook into.

  What `esdev` provides now is the generic half, the same for everyone:
  `import.meta.hot` and the update channel; `ctx.refresh`, the scheme the target
  named, handed to its plugins and present only while the loop is running that
  target hot; and the compiler's component registrations, on request
  (`jsx: { refresh: true }`, above).

  Gone from the binary: the React Fast Refresh pass, the built-in scheme name,
  and the `== "react"` comparison that selected either. The pass is now
  `plugins/react-refresh.mjs`, written out with the `react` template and named
  by its `esdev.json`. React has no privileges left — delete that entry and the
  template loses Fast Refresh exactly as any other framework would.

  **To migrate:** add the plugin to the target that names the scheme.

  ```json
  "web": { "entry": "index.html", "outdir": "dist",
           "refresh": "react", "plugins": ["./plugins/react-refresh.mjs"] }
  ```

  A `refresh` on a target with **no plugins** is now refused, since nothing
  could implement it — and a `refresh` that silently did nothing is a project
  whose components stop keeping their state one day, with the reason sitting
  unread in a config file.

- **Tests run one at a time.** **Breaking** for a suite that relied on its
  async cases overlapping.

  `test()` used to call the function where it was written, so every async case
  in a file ran at once. Two tests sharing a database, a temp directory, a port
  or a module global interleave under that, and the failure is a flake nobody
  can reproduce — and a `beforeEach` cannot exist at all, because there is no
  "before": the next test has already started. Suites were writing a couple of
  hundred lines of scheduler to get around it.

  Registration and execution are now separate: `test()` appends to a queue, and
  the queue drains one case at a time, in the order the file wrote them, with
  the lifecycle hooks around each.

  The host is told about a case when it is **registered** rather than when it
  starts, so the report stays complete: a case that never got a turn because an
  earlier one hung is reported as *"the test never started — a test before it
  never finished"*, separately from the one that is actually stuck.

### Fixed

- **A filter regex that will not compile is refused, not ignored.** `\0` is
  rollup's virtual-module prefix, so `/\0virtual/` is the first filter every
  ported plugin writes — and Rust's `regex`, which matches filters on the host
  side, has no `\0` escape. A pattern that failed to compile became one that
  admitted *everything*, so a `load` hook scoped to virtual ids claimed the real
  entry module and replaced its contents.

  `\0` and `\/` are translated now, and whatever is left that will not compile —
  a backreference, a lookaround — is refused at the declaration, naming the
  pattern and why, rather than silently matching the whole graph. A pattern that
  is neither a string nor a `RegExp` is refused for the same reason instead of
  being dropped from the list.

- **`order: "pre"` claims a file type ahead of a built-in pass.** Putting a
  project's plugins in one list with `esdev`'s own was necessary and not
  sufficient: `esdev:css-modules` filters on the module *id*, which is still
  `.css` after a `pre` plugin has turned the stylesheet into JavaScript, and it
  re-reads the file off disk rather than transforming the code it was handed
  (deliberately — an `@import` chain is a set of files, not one string). A
  Tailwind-style plugin's work was read, discarded, and the build failed with
  *"imports tailwindcss, which is not there"*.

  A transform now knows what the module *is* (`ctx.type`), and the CSS Modules
  pass steps aside when it is no longer CSS.

## [0.4.0] - 2026-08-18

### Added

- **`esdev upgrade`** replaces this binary with the newest `esdev` release for
  your platform — the same command `esrun upgrade` has always been, now shared
  between the two rather than living in one of them.

  `install.sh` places both binaries and has since `esdev` shipped, so the
  binary a developer runs all day was the one they could not update from the
  tool itself. Each side resolves *its own* release: the two are tagged
  separately (`esrun@0.25.0`, `esdev@0.3.0`), and asking GitHub for the latest
  release would answer with whichever was published most recently — which is
  how an `esdev` upgrade would otherwise download `esrun`'s archive.

### Fixed

- **An installed program can find its dependencies.** Running a file inside
  `node_modules` — `esdev node_modules/@acme/cli/src/cli.js`, the entry point of
  any installed CLI — stopped the project-root detection at the *package's* own
  `package.json`, so the hoisted dependency beside it was unreachable and no
  npm-installed program could run.

  The root is now the working directory itself rather than one derived from the
  entry file, here and in `esrun` alike; `--watch` watches that same directory.
  Running a program from somewhere else — or in a filesystem root, or in your
  home directory — is refused rather than silently rooted somewhere surprising.
  See the root [CHANGELOG.md](../../CHANGELOG.md) (DECISIONS D79).

- **`--install-types` reads `"packageManager"` before it looks for a
  lockfile.** Detection was lockfile-only, so a project that declares its
  manager — the corepack field, which modern toolchains write and which is
  there *before* anything has been installed — was installed with npm anyway,
  leaving a `package-lock.json` in a bun project.

  The order is now the declaration, then the lockfile, then whatever manager is
  actually on the machine, and npm last. "npm is always there" is the
  assumption that produces `npm: command not found` in a container that ships
  only bun. A declared manager that is not installed is still the answer: it
  prints the line to run rather than quietly using a different one, since
  installing with the wrong manager is what leaves the wrong lockfile. A field
  naming something unrecognised falls through to the lockfile rather than
  failing.

  `esdev create --install` is deliberately unchanged: it asks, and away from a
  terminal it installs nothing (D70).

- **A failed build no longer leaves half a deployment.** `esdev build` wrote
  straight into `dist`, and a whole-project build emptied `dist` first — so a
  build that failed halfway both destroyed the deployment that was working and
  left the fragments of the one that did not. The react static template shows
  it exactly: with no `node_modules`, the browser assets, the `index.html` and
  the prerender bundle are all written, and then the prerender step exits
  non-zero. What is left is a site whose pages were never rendered, and in CI
  it is what gets uploaded.

  The build now runs in a staging directory beside the project and moves its
  output into place only once every target and every `"then": "run"` step has
  succeeded. A failure removes the staging directory and says what it did not
  do — a `dist` that was left alone looks exactly like one that was rebuilt.

  A whole-project build still *replaces* each `outdir`, so nothing stale
  survives; that is the old up-front emptying, moved to the end where a failed
  build cannot benefit from it. `--target=`, a single entry and an `out` file
  are overlaid instead, leaving whatever else is in the directory alone.
  `esdev start` is unchanged: the dev loop writes in place, because it rebuilds
  into a directory a running page is being served from and fetches its hot
  updates out of it. See DECISIONS D78.

- **`esdev test --help` documented an API that no longer exists.** It said a
  test file "arrives with the globals already defined" and listed `test`,
  `assert`, `assertEquals`, `assertThrows` and `assertRejects`. Those became
  imports from `runtime:test` in 0.2.0, so a file written from the help failed
  on its first line with `ReferenceError: test is not defined`.

  The help now shows the import, says outright that nothing is ambient, and
  links the reference and the guide. A test writes the file the help shows and
  runs it, so the claim cannot go stale again in silence.

- **The same five declarations were copied into every template's
  `src/esdev-env.d.ts`**, where they were worse than stale: `tsc` accepted a
  file the runner rejects. Deleted — `@opentf/esrun-types` has declared
  `runtime:test` since it shipped, and every template already depends on it.

### Changed

- **`esdev start` refuses the run flags it was dropping.** `--timeout`,
  `--max-heap`, `--env-file`, `--env-override` and `--import-policy` were
  accepted and then ignored: `start` does not run your program, it runs a
  build's output as a child process under `esdev.json`'s `permissions`. Each
  now fails with where it belongs instead. `--shutdown-grace` is unchanged —
  it is the one that applies, bounding the drain on a restart.

- **`--help` is a map, not a manual.** `esdev --help` was 118 lines and its four
  subcommands another 328 between them — the port-selection rule, the reload
  channel, the `esdev.json` target keys, the case for `--dts-bundle`. All of it
  is on the site, where it is one copy and can be corrected without shipping a
  binary, so each `--help` keeps the grammar, the flags and the shape of the
  command, and ends with the URL that has the rest.

  | | before | after |
  | --- | --- | --- |
  | `esdev --help` | 118 | 60 |
  | `esdev create --help` | 60 | 34 |
  | `esdev start --help` | 80 | 29 |
  | `esdev build --help` | 156 | 55 |
  | `esdev test --help` | 32 | 26 |


- **A template is a scaffold, not a demo.** Every template shipped an
  application inside it — a blog with three posts and a dynamic route, a task
  manager with a store and hand-written validation, an item list with add and
  remove, a `Result` type and a `retry` with backoff — and all of it had to be
  read and deleted before the project could become the one it was created for.

  What `esdev create` writes now is a project that runs and **one page**: its
  name, a line saying what it was built with and who it comes from, the file to
  edit, and three links. The `api` template answers the same in JSON on
  `GET /`. `styles/theme.css` and `styles/app.css` are byte for byte the same
  file in the `react` and `vanilla` templates.

  What is kept is what a project needs on its first day and would otherwise be
  assembled by hand: the route table, the layout, the error boundary that
  renders a real 404, one render shared by the server and the static build, Fast
  Refresh, the `URLPattern` router that tells a 405 from a 404, the security
  headers, the `SIGTERM` drain, the JSON access log, and a permission line that
  is already narrow. Each template keeps one test, since `esdev test` exits
  non-zero when it discovers nothing. See DECISIONS D76.

### Fixed

- **A scaffolded project gets the current type definitions.** Every template
  pinned `@opentf/esrun-types` to `^0.1.0`, and a caret on a `0.x` version does
  not cross the minor — so every project created after the definitions shipped
  0.2.0 quietly kept resolving 0.1.x, and typed against a runtime older than the
  binary sitting beside it. The templates ask for `latest`, which is what
  `esdev --install-types` has always done: it names no version, so the two doors
  into the same package now agree. Two tests hold it — one that no template
  pins the package, one that every template depends on it at all.

## [0.3.0] - 2026-08-17

### Changed

- **Hot replacement is on by default.** `esdev start` patches a changed module
  into the running page; `--no-hot` goes back to reloading it. What it costs is
  development-only — rolldown's dev mode forces treeshaking off, so the react
  template's dev bundle goes from 870 KB to 1.45 MB and a rebuild costs about
  20 ms more — and what it buys is the state in the page surviving a save.
  Nothing shipped is affected either way.

- **Two pages of the same app both stay hot.** A patch is trimmed against what
  has already been delivered, so a tab opened later — holding a bundle rather
  than the patches before it — could not apply the next one and reloaded itself.
  A page connecting now clears that record, so the next patch carries what the
  newest page needs and the older ones are handed a superset, which is what their
  own graph walk is built to filter. Verified with two tabs and two edits: both
  keep their state.

### Added

- **React components keep their state across an edit.** `esdev start --hot` on
  the react template now applies React Fast Refresh: change a component and the
  page shows it without reloading, with `useState`, scroll position and anything
  typed into a form still there.

  The split is deliberate. esdev owns the transform (oxc implements it, rolldown
  exposes it) and the per-module wrapper, because both need the module graph.
  The React half — the runtime bootstrap that must run before React itself
  loads — is `src/refresh.ts` **in the template**, where React was chosen. And it
  is an ordinary consumer of the generic API: the refresh runs as an
  `import.meta.hot.accept` callback, so a framework with its own scheme writes
  its own pass against the same contract.

  Enabled per target with `"refresh": "react"` in `esdev.json`, applied in the
  dev loop only. An unknown name is refused rather than ignored, because a
  scheme that is silently dropped is a project whose components stop keeping
  their state one day with the reason sitting unread in a config file.

  Verified in Chrome on both template modes: a counter clicked to 3 still reads
  3 after the component's markup is edited, with the edit visible and no reload.

- **`esdev start --hot` hot-replaces a changed module instead of reloading the
  page.** A module that says `import.meta.hot.accept(cb)` is a boundary: when it
  or anything it imports changes, esdev computes a patch, the page loads it,
  walks its own import graph up to that boundary, drops the affected modules from
  its cache, re-runs the boundary and calls the callback — with no page load, so
  scroll position, open dialogs and typed-in state all survive.

  When nothing on the way up accepts, the page reloads. That is not a failure: a
  module that says nothing about how to replace it has not earned being replaced,
  and reloading is the correct answer. The same fallback catches a patch that
  fails to load or throws while applying.

  The bundle is built in rolldown's dev mode, so modules are registered with a
  runtime rather than scope-hoisted into one another. The boundary walk is
  esdev's: rolldown's patch assembler is explicit that it ships no driver —
  *"the client walks its own graph, removes from its cache, and re-runs from the
  factory map"* — so what a patch does on arrival is entirely the consumer's to
  decide.

  It stays a flag, for now, because dev mode forces treeshaking off: the react
  template's dev bundle goes from 870 KB to 1.45 MB and a rebuild costs about
  20 ms more. And the react template has no `accept` in it yet, so a component
  edit still reloads — React needs Fast Refresh to keep hooks state across a
  swap, which is a plugin rather than anything esdev knows about.

  **One session, not one per page.** rolldown's API takes a list of clients with
  a ship map each, because two tabs opened at different times can need different
  patches. esdev keeps one and broadcasts: the pages of a dev loop have almost
  always loaded the same bundle, and the one that has not reloads itself when the
  patch does not fit its graph.

  Verified in Chrome, driven over CDP: a marker set on `window` survives the
  update when a module accepts (proving no reload happened) and is gone when
  none does (proving the fallback fires), with the DOM correct either way.

- **`import.meta.hot` is a full API, and it is framework-agnostic.** Nothing in
  it knows what React is; any framework — or none — hooks into the same surface:

  | | |
  | --- | --- |
  | `accept()` / `accept(cb)` | re-run this module in place |
  | `accept(dep, cb)` / `accept([deps], cb)` | re-run *that dependency* and notify me with its new exports |
  | `signal` | an `AbortSignal`, aborted just before this module is replaced |
  | `keep(key, make)` | a value made once and returned on every replacement after |
  | `dispose(cb)` / `data` | the conventional pair, for integrations ported from elsewhere |
  | `decline()` | refuse replacement; any change reaching this module reloads |
  | `invalidate()` | give up and try again from this module's importers |

  **`signal` is the one worth knowing about.** The commonest hot-reload bug
  anywhere is a listener or timer added on every re-run and removed on none, so
  the twentieth save has twenty; the usual cure is remembering to hand-write a
  `dispose` that undoes what the module did. The platform already solved this
  generally, and the whole platform already takes the solution as an argument:

  ```js
  addEventListener("resize", onResize, { signal: import.meta.hot.signal });
  ```

  That line is correct under replacement with no HMR-specific code at all, and
  it still works in a production build, where the signal is never aborted.
  `fetch`, observers and any library taking a signal come along for free.

  **`keep` is one call site where `data` is two.** Carrying state across a
  replacement conventionally means writing a bag in `dispose` and reading it at
  the top of the module — two places that must agree, failing silently when they
  do not. `const cache = import.meta.hot.keep("cache", () => new Map())` is made
  once and returned every time after.

  `accept(dep)` re-runs **the dependency**, not the acceptor — the contract the
  ecosystem shares, and the one rolldown builds to: a patch for `accept(dep)`
  ships the dependency's factory and not the acceptor's, so re-running the
  acceptor is not merely wrong but impossible.

  Every form verified in Chrome: `keep` surviving two consecutive replacements,
  exactly one listener firing after three module instances registered one
  (the earlier two aborted by `signal`), `dispose` writing into `data` for the
  next instance, `accept(dep)` receiving the dependency's new exports, and
  `decline()` reloading.

### Changed

- **The dev loop's reload channel is a WebSocket at `/@esdev/hmr`.** It was
  server-sent events at `/@esdev/reload`, which was the right shape for a channel
  carrying one word and the wrong one for what it is being built to carry.

  Two reasons, neither about today. A hot update is a **module's source** —
  multi-line JavaScript — and SSE is a line protocol, so every patch would be
  JSON-escaped or split across `data:` lines on the hot path for ever. And SSE
  **runs out of connections**: HTTP/1.1 caps a browser at roughly six per origin
  and a stream holds one open for as long as the page is, so the seventh tab of
  your own app simply stops updating with nothing saying why.

  The handshake and framing cost nothing here — `--inspect` already speaks
  WebSocket in its server role, in this binary, on this accept loop. What it cost
  is the reconnect loop `EventSource` provided for free: the injected client now
  backs off from 250 ms to a 5 s ceiling, because a dev server being restarted is
  the ordinary case rather than a failure.

  The message is typed (`{"type":"reload"}`) rather than a bare word, so the CSS
  swap and the module patch that follow are new variants rather than a new
  protocol. A plain `GET` of the endpoint answers `426 Upgrade Required` rather
  than hanging up, since that URL is what somebody reaches for to check whether
  the dev server is alive.

- **A save reaches the browser in 238 ms instead of 340 ms.** 120 ms of that
  cycle was `--watch`'s settle window — a fixed wait after every change, so that
  one editor save (a truncate, a write, sometimes a rename) became one rebuild
  rather than three. The window is right; its length was not. Those events land
  within a millisecond or two of each other, so **30 ms** clears them with an
  order of magnitude to spare, and the window still restarts on every event, so
  it is a lull rather than a delay. An editor that straggles past it costs one
  wasted rebuild, and a build that fails changes nothing.

  A burst can no longer hold a rebuild off indefinitely either: anything writing
  a steady stream into a watched tree — `git checkout`, an install, a formatter
  walking the project — used to reset the lull on every event, so the rebuild
  never came and the dev loop looked like it had stopped working. It is now
  capped at 500 ms: build what is there, and build again when the stream ends.

  Measured on the react fullstack template with a release binary: cold
  `esdev start` to serving a real request is 237 ms, a full build of both
  targets is 153 ms, and save-to-server-back-up went from 340 ms to 238 ms.

- **The react template answers `HEAD` with the headers and stops.** Every route
  is handled as though it were `GET` and the body is dropped once, centrally
  (`src/http/method.ts`), rather than in each handler.

  This is not a hang being fixed — the server never writes a body for a HEAD,
  and an earlier report that the template's `HEAD /` and `HEAD /assets/…` timed
  out does not reproduce. What it changes is what the handler leaves behind: an
  asset response is an open file being streamed, and handing it to a server that
  will not read it kept the handle alive until the collector got to it. It is
  now cancelled at once, and a page that nobody will receive is not rendered.

### Fixed

- **A release build clears the directories it owns.** Output filenames are
  content-hashed, so `app-1a2b.js` becoming `app-9f8e.js` left the old one in
  `dist` for ever — and beside it whatever `esdev start` had written, which is
  *not* hashed and so was never overwritten either. What shipped was every build
  the directory had ever seen, and a stale URL still reached a version of the app
  nobody was testing.

  Only `outdir` directories are cleared, only when **every** target is being
  built, and never for the dev loop. `--target=web` writes into a directory it
  may share with another target's output, and clearing it would delete a bundle
  that run is not going to write again; `esdev start` rebuilds on a keystroke
  into stable filenames, where nothing accumulates and clearing would break the
  page being served. An `outdir` that holds the project itself is refused rather
  than emptied.


- **Two projects can be in development at once, and `--port` is the port you
  open.** Both halves of that were wrong before. `esdev start --port` moved
  esdev's own endpoint out of the way when it was busy, but the *application's*
  port never moved: both projects ran their server on whatever `esdev.json`
  granted — 8080, because that is what the template came with — and the second
  one died on a bound address. And `--port`, the name every dev server uses for
  the address you type, pointed at the endpoint, which is the one thing here
  nobody types.

  `--port` is now **the port you open**: your server's when the project has a
  `run` target, esdev's listener when it does not. It follows the rule the
  endpoint always had — named is a promise and fails if taken, unnamed takes the
  port your `listen` grant names, or a free one if that is busy, and prints what
  it settled on. The child is handed it as `PORT`.

  esdev's own endpoint on a fullstack project is no longer a port you deal with:
  it carries one message to the page, the build writes its address into the page,
  and it takes a free one. There is no flag for it, because there is nobody to
  type one.

  A port is moved only for a project that says enough for the move to be safe,
  and both halves are grants that were already being written: `"listen":
  ["8080"]`, one port and no more, so there is a port to move; and `"env":
  ["PORT"]`, so the server can be told which one it got. Anything shaped
  differently is left exactly as it was. The rewritten grant is the same
  capability with a different number and never a wider one, so development still
  runs under the deployment's grant.

  **Breaking:** `--port` and `"start": { "port": … }` mean the application's port
  now for a project with a `run` target. Pinning esdev's endpoint is gone; it
  always takes a free one.

### Changed

- **The `react` template is one project again, not three.** It used to build a
  server, a browser bundle *and* a prerendered site from every scaffold, and
  leave you to work out which of the three you were shipping. It now comes in
  two modes, asked at `esdev create` and settled once:

  | `--mode=static` | `--mode=fullstack` |
  | --- | --- |
  | No server. `npm run build` prerenders every route to `dist/static/`; `npm run build:spa` builds the shell alone | A server of its own in `src/server.tsx`, rendered per request |
  | Nothing to grant, because nothing runs in production | `read`/`env`/`listen`/`signals`, the same in development and production |
  | Deploys to any static host | Deploys `dist/`, run under `esrun` |

  SSG and SPA are both `static`, on purpose: which one a site wants is a
  deployment decision that can change with the content, and both come out of the
  same routes and components with no file edited. Which routes are prerendered is
  `staticPaths()` in `src/paths.ts`; a route left out of it is rendered in the
  browser instead, so a mostly-static site with a couple of client-only routes is
  the same project with a shorter list.

  Neither mode carries the other's files. A static project has no `src/server.tsx`
  and no response-header module; a fullstack project has no prerender step. The
  static build no longer runs in `esdev start` either — rendering every route on
  every keystroke buys nothing when the components and loaders are the same ones
  already on screen.

  A template directory may now hold `_mode/<name>/`, and a scaffold is everything
  outside it plus one mode's files with the prefix stripped; an overlay may add a
  file or replace a shared one. `esdev create --list` names the modes and what
  each one weighs.

- **Every template's `npm run typecheck` works on a fresh scaffold.** All four
  declared the script but not what it needs, so the first run reported a dozen
  unresolved `runtime:*` imports until somebody found `esdev --install-types` in
  a README — and on `api`, `lib` and `vanilla` there was no `typescript` either,
  so it depended on one being installed globally. Both are dev dependencies now,
  and `@opentf/esrun-types` is named in `tsconfig.json`'s `types`.

  `api`, `lib` and `vanilla` therefore have a `devDependencies` block where they
  had none. Their claim is unchanged and is now stated as what it always meant:
  **nothing they ship depends on anything.** A compiler and a set of `.d.ts`
  files are not loaded by anything that runs, and the alternative was a
  `typecheck` script that did not work.

- **Templates build minified.** `npm run build` passes `--minify`; the
  unminified build is still there as `npm run build:debug` when you want to read
  the output. What a starter's `build` script produces is what people deploy.
  `lib` is the exception and stays unminified — a published package is minified
  by whoever bundles it, if at all.


- **esdev's own output is coloured.** Green for something produced (`built`,
  `bundled`, `created`), cyan for somewhere to go or look (a path, a URL), bold
  for a line to type, dim for the part you skip when skimming. Four colours, one
  meaning each, so a build report or a dev-loop banner can be read at a glance
  without being read.

  Nothing is red: red belongs to the error block, and a status line reaching for
  it is a status line competing with an actual failure. The gate is per stream
  rather than per process — `esdev build > build.log` writes a plain log while
  the `esdev start` in the next terminal stays coloured — and `NO_COLOR` turns it
  all off.

- **`esdev create` asks with a menu you can arrow through.** The questions were
  a numbered list read off stdin, which made choosing a template an exercise in
  counting lines and typing a digit. They are now drawn with
  [ratatui](https://ratatui.rs) into an **inline viewport**: arrow keys (or
  `j`/`k`) to move, `1`–`9` to jump, Enter to take it, Esc to cancel.

  Inline, and deliberately not the alternate screen. The menu is drawn where the
  cursor already is and is replaced in place by a single line naming the answer,
  so what is left in the scrollback afterwards is a transcript of what was asked
  and what was chosen. A full-screen TUI would hand the terminal back empty and
  take the record of a command that writes a project to disk with it.

  Esc at "which template?" now **cancels**, writing nothing and exiting zero —
  it does not silently mean "the default". Esc at "install the dependencies?"
  means no install, because by then the project is already on disk and
  cancelling the question is not cancelling the project.

  Nothing about the non-interactive path moved: the gate is still a TTY on both
  stdin and stderr with no `CI` set, every question still has a flag, colour
  still honours `NO_COLOR`, and a terminal that refuses raw mode still gets the
  numbered list.


- **There is one place a build becomes the bundler's options.** Both
  divergences above were two translations of the same idea drifting apart, and
  finding them by reading the two side by side is not a way of finding bugs.
  `crates/dev-cli/src/bundler.rs` now owns the option types every build in this
  binary describes itself with, and is the only place in the crate that
  constructs a `BundlerOptions`. The `build` subcommand, the client bundle it
  emits for a browser, and `runtime:build` fill in the same struct and call the
  same translation. It is checkable by grep rather than by reading.

  Two things follow, both visible:

  - **`--lib` is a target rather than a flag.** The library rules — assert no
    condition, define nothing, preserve modules, externalise everything that is
    not its own source — sit with the other targets instead of as `if lib`
    branches through a long struct literal.
  - **A `--lib` build is neutral even when its target says `browser`.** It used
    to take the browser platform, whose aliasing follows a package's `browser`
    field — which bakes the consumer's environment into a published package, the
    same objection that already stopped a library asserting conditions or
    defining `NODE_ENV`.

- **This toolchain's own passes go through the plugin contract too.** When
  `runtime:build`'s plugin API stopped being the bundler's, the claim was that
  our passes and a guest's plugin are the same kind of thing. They were not:
  the CSS Modules pass was written against **rolldown's** trait, and the
  contract had exactly one implementation — which is the same as saying it had
  never been tested.

  It is a `Pass` now, like a plugin declared in JavaScript is a `Pass`. One
  list, one order, one set of filter rules, one adapter — so swapping the
  bundler is one file for both kinds of pass, where before it was one file plus
  every pass we had written ourselves. The contract moved to the crate root,
  since it was never guest-only and a path is a claim.

- **`esdev start` restarts the server only when the build changed something it
  reads.** Every rebuild used to `SIGTERM` the child and start it again, so
  editing a stylesheet or a browser component cost every open connection, every
  warm cache the process had, and a window where requests were refused — to
  deliver a server byte for byte identical to the one just stopped.

  The build still runs on every change and the browser is still told to reload.
  What is conditional is the restart: after the build, esdev compares the
  **contents** of the run target's output and everything the build left beside
  it — its own `server.js`, the `index.html` a server splices its render into,
  a manifest it loads at startup — against what was there before. The client
  asset directory is excluded, because nothing in it is read by the server; the
  browser fetches it over HTTP from a URL that has not changed.

  Contents rather than timestamps, because a rebuild rewrites `server.js`
  whether or not a byte of it changed, and a modification time would say
  "different" every time. A child that is *not* running is started whatever the
  comparison says — the developer is fixing the reason it stopped.

- **`esdev start` finds a free port instead of refusing to start.** It bound
  5173 and failed if anything was there, so a second project in a second
  terminal — an ordinary afternoon — stopped at
  *cannot bind 127.0.0.1:5173: Address already in use*.

  A port nobody named is a convenience, so it now takes 5173 when it is free
  and any free port when it is not, printing the one it settled on:

  ```
  esdev: serving dist on http://127.0.0.1:39481
  esdev: 5173 was taken; use --port to pin one
  ```

  A port that **was** named — `--port=8080`, or `"port": 8080` in `esdev.json`
  — still binds that one or fails, and the failure now says what to do about
  it. Moving quietly off a port somebody chose would leave a bookmark, a proxy
  rule or a second terminal pointing at whatever is already there. `--port=0`
  asks for a free one explicitly, which is what a script reading the printed
  URL should pass.

  This is esdev's own endpoint — the one serving your output and the reload
  stream. The port your *server* listens on is your server's: it reads it from
  `PORT`, and `esdev.json`'s `permissions` is what says which one it may have.

### Fixed

- **A guest build resolves what `esdev build` resolves.** `esdev build` asserts
  the `worker` condition and the `module`/`main` fields; `runtime:build`
  asserted neither unless the caller named them, because it translated its
  options in a second place. Two silent wrong builds came out of that gap, and
  neither failed anything at build time:

  - A package with an `exports` map handed the guest its **`node:` build**
    where the subcommand got its Web build — `react-dom/server` resolving to
    the `node:stream` implementation instead of the Web Streams one, which
    fails at runtime, in the request.
  - A package too old to have an `exports` map **did not resolve at all**, and
    survived into the output as a bare `import { x } from "legacy"` — a bundle
    referring to something that is not there.

  The defaults now live in `crates/dev-cli/src/resolve.rs` and all three build
  paths read them from it: the subcommand, the client bundle it emits for a
  browser, and `runtime:build`. `platform: "neutral"` (the default) asserts
  `worker`, `"browser"` asserts `browser`, and `"node"` leaves resolution to the
  bundler's own knowledge of Node.

  A caller's `resolve.conditionNames` **append** to the target's — matching what
  `--conditions` does on the subcommand, so naming one cannot cost you `worker`
  — and `resolve.mainFields` **replace** them, because there is one ordered list
  and a caller who writes one means it.

  This is the second divergence of its kind between the two paths, after
  `runtime:build` not installing the CSS Modules pass. Both were found by
  looking, not by a test failing — which is why the fix went further than the
  defaults, under **Changed** above.

- **An `@import`ed stylesheet is watched.** A `.module.css` that `@import`s
  another file, or reaches one through `composes … from`, depends on a file
  **nothing imports** — the reference is inside the CSS, and only this project's
  own CSS bundler follows it. Neither reached `watchFiles`, so a `--watch` save
  to one of them rebuilt nothing and the page kept the rules it had.

  This is the first thing the contract caught rather than the reviewer: a hook
  *returns* what it depends on, and the pass had nowhere to put it while it was
  written against a trait where you call `this.addWatchFile()` or you forget.
  It forgot.

## [0.2.0] - 2026-08-15

### Added

- **Three `runtime:` modules that only this binary has.** `esdev` now serves
  modules of its own on top of the runtime's, through the seam `esrun` 0.25.0
  added (`Runtime::register_module`). They are development surface, so `esrun`
  does not merely leave them unwired — it does not contain them, and importing
  one there fails at load with *unknown built-in module*. Ship the artifact, not
  the tool that built it.

- **`runtime:build` — the bundler, callable from a program.**
  rolldown is already inside `esdev`; it is what `esdev build` runs. What was
  missing was a way for *guest code* to reach it — and without that, a
  framework's dev server has to import a bundler from npm, which is a napi
  addon this runtime does not load, so the dev server has to be a Node program.

  ```js
  import { build } from "runtime:build";

  const bundle = await build({
    input: "app/main.jsx",
    external: (id) => id.startsWith("/__route/"),
    plugins: [mdx()],
  });
  const { output, watchFiles } = await bundle.generate({ codeSplitting: false });
  serve(output[0].code);            // never written to disk
  ```

  **Real plugin hooks, taking real functions.** `buildStart`, `resolveId`,
  `load`, `transform`, `buildEnd`, with rollup's arguments and rollup's `this`
  — `this.resolve()`, `this.addWatchFile()`, `this.emitFile()`, `this.warn()`.
  Piping source through a subprocess was considered and cannot work:
  `resolveId` + `load` serve modules that exist on no disk, and there is
  nothing to pipe.

  **`watchFiles` comes back with the output**, including whatever a plugin
  declared it depends on. Paired with `runtime:watch`, that is what lets a dev
  server drop the three cached chunks a save invalidated and keep the other
  thirty-seven.

  The bundler runs on a thread of its own with a multi-threaded runtime, so its
  parallel graph walk is not serialized onto the isolate's thread. Hooks cannot
  follow it there — an isolate belongs to one thread — so a hook posts a request
  and waits, and the guest's pump answers it. Several are in flight at once.

- **`runtime:build`'s plugin system is the project's own, not the bundler's
  passed through.** A `runtime:` module is a versioned contract, and an API
  defined by a third party's Rust trait moves when that trait moves — a hook
  renamed in a bundler's patch release would be a breaking change in this
  runtime's standard library. There is now a contract layer between them:
  rolldown is an implementation of it, named in exactly one file.

  ```js
  const mdx = {
    name: "mdx",
    transform: {
      filter: { id: /\.mdx$/ },
      handler(code, id, ctx) {
        const { js, meta } = compile(code, id);
        return { code: js, type: "jsx", dependsOn: [meta] };
      },
    },
  };
  ```

  Five hooks — `start`, `resolve`, `load`, `transform`, `end` — and four things
  in the shape are deliberately not rollup's:

  - **A filter is declarative**, and matched on the host's side. In rollup a
    hook returning `null` costs a function call; here it costs a round trip into
    the isolate, so an unfiltered `transform` is one crossing *per module in the
    graph*. A pattern the host cannot evaluate stops filtering rather than
    failing — excluding modules a plugin was meant to see is the expensive way
    to be wrong.
  - **Dependencies are returned** (`dependsOn`), not declared by calling
    `this.addWatchFile()`. A call you can forget produces a build that serves
    stale output; a field of the value you return does not fail that way.
    Relative paths resolve like any other path in a run.
  - **A virtual module says `virtual: true`**, instead of being signalled by a
    NUL byte glued to the front of its id.
  - **The context is the last argument, not `this`** — so an arrow-function
    handler keeps it.

  Plus `order: "pre" | "post"`, which rolldown supports and was never surfaced.

  **Breaking, and with no fallback:** a hook is an object carrying a `handler`.
  Rollup's bare-function shorthand is refused, with a message saying what to
  write instead; so is a misspelled hook name (which names the one it was
  nearly), a filter on a whole-build hook, a `code` filter on anything but
  `transform`, and an unknown `order`. All of it is checked by `build()`, at the
  line that wrote the declaration.

- **`runtime:test` — the test API, imported rather than ambient.**

  ```js
  import { test, assert, assertEquals, assertThrows, assertRejects } from "runtime:test";

  test("adds", () => assertEquals(add(2, 3), 5));
  ```

  **Breaking:** `test` and the four assertions are no longer globals. A test
  file must import them. Every test file in this repository and in the `esdev
  create` templates was updated; the API itself is unchanged.

  They were globals prepended to each test file's own source, folded onto a
  single physical line so the file's line 1 stayed line 1, with an epilogue
  appended to await and report. Three things were wrong with that, and an
  import fixes all of them: this runtime hands out no ambient names anywhere
  else; only the *entry* was wrapped, so a shared `test-helpers.ts` beside the
  test file could not call `assertEquals`; and there was nowhere to declare
  them, so a `.ts` test file referenced five undeclared names and `tsc
  --noEmit` failed on a suite that ran perfectly.

  The tally moved into the host, which removes the appended epilogue too — so
  what runs is now byte for byte the file on disk. Two things follow:

  - **`esdev app.test.ts` works on its own.** A test file is an ordinary
    module, and any run that imported `runtime:test` prints the same report.
  - **A test that never settles is a failure**, reported as *"the test never
    finished"*. The epilogue used to `await` every pending promise, so such a
    test hung the file forever.

- **`runtime:watch` — file-change events in guest JS.** A dev
  server cannot answer a save the way `esdev --watch` does. That watcher
  `SIGTERM`s the program and starts another, which is right for *rerun this
  script* and wrong for a server holding forty compiled chunks, an open
  websocket to a browser and a warm compile server: it has to **stay up** and
  drop only what changed.

  ```js
  import { watch } from "runtime:watch";

  const changes = watch(["app", "lib"], { recursive: true });
  for await (const { kind, path } of changes) {
    invalidate(path);
    for (const dep of rebuild()) changes.add(dep);   // the set grows as it runs
  }
  ```

  The watch set is mutable because it is not knowable up front — which files a
  bundle depends on is known only after it is built. Events are debounced per
  path, so one editor save is one event rather than three, and what a burst adds
  up to is reported honestly: create-then-write is a create, and the
  remove-then-create every editor does on save is a modification.

  Gated on `FileRead` and scoped by the same `--allow-read` list as reading —
  watching a directory tells you which files exist and when they are touched.

- **`esdev create` asks on a terminal** (D70): which template, and whether to
  install. Away from one — a pipe, a CI job, anything with `CI` set — it takes
  the defaults and says nothing, because a prompt in a script is a script that
  hangs.

  Every question has a flag, so nothing is only reachable by answering one:

  ```sh
  esdev create my-app --template=api --install=bun
  esdev create my-app --yes            # defaults, no questions
  esdev create my-app --no-install
  ```

  Only package managers this machine actually has are offered, detected by
  running `--version` rather than walking `PATH`. Unattended runs still install
  nothing: D64's objection was to *guessing* which package manager a project
  uses, and asking resolves that at the root.

- **Three more templates** for `esdev create`, so it offers a real choice:

  | | |
  | --- | --- |
  | `api` | A JSON API — routing on `URLPattern`, validation, error mapping. **No dependencies**, 9.9 KB bundled |
  | `vanilla` | TypeScript and the DOM, no framework |
  | `lib` | A publishable package — module tree preserved, `.d.ts` emitted, `exports` wired |

  Each is dependency-free, which is the point of an `api` template on a server
  runtime: `URLPattern`, `Request`, `Response` and `crypto.randomUUID()` are all
  web standards this runtime already has, so a router is a table and a loop. Its
  permission line grants **no filesystem at all** — not even read.

  Having no React also means nothing reaches CommonJS, so `esdev test` can run
  every module: 20 tests in `api`, 9 in `lib`, covering the router's 405-with-
  `Allow`, the error-to-response mapping, the validation, and the retry
  backoff's abort path.

  A new end-to-end test scaffolds every dependency-free template and runs its
  own suite and build, because a template is a project nobody builds until
  somebody depends on it.

- **`esdev build` bundles stylesheets** (D67). A `<link rel="stylesheet">` in an
  `index.html` target is now an entry, the way a `<script type="module">`
  already was: it and everything it `@import`s become one hashed file, and a
  relative `url()` is followed so fonts and images travel with the stylesheet
  instead of arriving as 404s once it moves to `/assets`. `--minify` drops
  comments and collapses whitespace.

  The hash is computed **after** `url()` substitution, so editing an `@import`ed
  file changes the entry's URL. Hashing the source would have left a stale-cache
  bug visible only in production.

  **It adds no dependency.** The pipeline is a real implementation of
  [CSS Syntax Level 3](https://www.w3.org/TR/css-syntax-3/) — tokenizer, syntax
  tree, parser, printer — with the `@import` and `url()` passes on top. It stays
  small because it follows the spec's own two-layer design: the *generic*
  grammar every rule obeys is closed and complete, so the tree has no selector
  or media-query type and holds `@property`, `@container` and whatever ships
  next year without knowing anything about them.

  It is **lossless by construction**: every token keeps its verbatim text and
  nothing is discarded, so `print(parse(x)) == x` for any input, valid CSS or
  not. A pass that does not touch something cannot change it — which is the
  guarantee that makes CSS tooling safe to run in a build.

  lightningcss was used first and withdrawn: it is MPL-2.0, and while that
  licence never reached `esrun` or anyone's application, it would have meant a
  standing seven-crate copyleft exception in a `deny.toml` that opens by
  refusing copyleft. `deny.toml` is back to `exceptions = []`.

- **CSS Modules** (D69). A `*.module.css` imported from JavaScript is scoped to
  the file that declares it, and the import resolves to the mapping:

  ```js
  import styles from "./Button.module.css";  // { button: "button_a1b2c3d4" }
  ```

  Class selectors, id selectors and `@keyframes` are renamed — the animation
  references too, since a renamed `@keyframes` with an un-renamed reference is
  an animation that silently stops running. `:global(…)` opts a name out, and
  the wrapper is removed on the way through.

  The scoped name is derived from the file's **path**, not its contents or a
  counter. So a server build and a browser build arrive at the same name without
  talking to each other (SSR hydrates cleanly), two machines building one commit
  agree, and editing a component does not rename its classes.

  Every module's CSS is collected into **one hashed stylesheet, linked from the
  document** — never injected from script. Injecting costs a flash of unstyled
  content, puts styling behind script execution, and needs
  `style-src 'unsafe-inline'`, which the React template's own policy does not
  grant.

  **`composes`** reuses a class without repeating its rules, in all three forms
  — `composes: a b` (same file), `composes: a from "./x.module.css"`, and
  `composes: a from global`. The mapping's value becomes a list of class names
  and the element carries all of them. It is **transitive**: composing a class
  that itself composes gets the whole chain, because a class only styles an
  element that actually carries it. Cycles are refused, and a composed module's
  rules are emitted even though nothing imported it.

  **A plain `import "./x.css"`** — anything not `.module.css` — is emitted
  unscoped. That is what third-party stylesheets need: a library's own
  JavaScript emits its class names as hardcoded strings, so scoping them would
  rename half of a contract the library has with itself. The alternative, copying
  the file out of `node_modules` and `<link>`ing it, goes stale on the next
  upgrade.

  Not included, deliberately: syntax lowering and vendor prefixing (nesting and
  `color-mix()` are supported across the target browsers), value-level
  minification, and per-file typed class names.

- **`esdev` is installed by the one-liner.** The install script places both
  binaries into `~/.es-runtime/bin`; `--only=esdev` (or `$env:ES_RUNTIME_ONLY`
  on Windows) installs just this one, and `ESDEV_VERSION` pins it.

  `esdev` has **no self-upgrade**, unlike `esrun` — re-run the installer. A
  development tool that rewrites its own executable is one more thing to go
  wrong on the machine where re-running one command is the fix.

### Changed

- **`esdev test` assertions compare properly** — and the second argument to
  `assertThrows` / `assertRejects` is now what the error must be, not a label.

  `assertEquals` compared through `JSON.stringify`, which *throws* on a
  `BigInt` — so on this runtime the assertion an int64 test most needs could
  not be written. It also rendered a `Uint8Array` as `{"0":1,"1":2}` instead of
  comparing bytes, and cared about object key order. It now walks the values:
  `BigInt` and `NaN`, typed arrays and `ArrayBuffer` as bytes, `Map` and `Set`
  by contents, objects by key set, and cycles terminate.

  ```ts
  assertEquals(reader.int64(), -9223372036854775808n);   // used to throw
  assertThrows(() => s.decode("M", bytes), /field number 0/);
  assertThrows(() => validateTitle(body), HttpError, "accepted a string");
  ```

  **Breaking:** `assertThrows(fn, "TypeError")` used to treat `"TypeError"` as
  the text to print on failure, so it asserted nothing — any throw passed. It
  is now the expectation (an error name or message substring, a `RegExp` over
  the message, or a constructor for an `instanceof` check), and the failure
  label moved to the third argument. Every call site in this repository was
  already written the new way.

- **`.ts` stack traces name the line you wrote.** The harness is folded onto one
  physical line so it cannot renumber the file, but it was being prepended
  *before* type-stripping — and the stripper re-prints through oxc's codegen,
  which unfolded it again. An assertion on line 2 of a `.ts` file reported line
  44. Stripping now happens first. `.js` was never affected.

- **The `react` template is rebuilt on react-router 8** (D68), and is now a
  starting point rather than a demonstration. A real route table — nested
  layouts, dynamic segments, per-route loaders, error boundaries — read by the
  server, the browser and the prerender step alike. A loader that throws a
  `Response` produces a real status, so a 404 is a 404 rather than a 200 that
  says "not found".

  The server does what a deployed one has to: a Content-Security-Policy with a
  per-response nonce (the one inline script is admitted by nonce, never
  `'unsafe-inline'`), `nosniff`, `referrer-policy` and `permissions-policy`;
  `SIGTERM` closes the listener and drains before exiting; immutable caching on
  hashed assets; a `/healthz` that touches neither router nor data source; and
  one JSON log line per request.

  Fixed along the way, all silent: `globalThis.process.env.PORT` was read on a
  runtime that has no `process` global and was `undefined` forever (it is now
  `env` from `runtime:process`, granted `--allow-env=PORT`); a 404 shipped the
  client bundle with no matching route, so `entry.client.tsx` threw into the
  console of every 404; `render.tsx` claimed to stream the head first and did
  not; and the README named a test file that did not exist and told the reader
  to run `<pm> install`, a placeholder nothing substitutes.

  `src/serialize.ts` is gone — `<StaticRouterProvider>` emits the hydration
  script itself, escaped, and takes the nonce. The template's most
  security-sensitive hand-rolled code is now the library's.

- **`esrun` now grants nothing by default; `esdev` still grants everything**
  (DECISIONS D65). Along with the modules above, this is where the two binaries
  differ, and it is deliberate: a dev loop that dies on an unnamed capability at
  every `--watch` save is the friction D59 put this binary here to absorb. The
  flags, scope lists, rules and error text are one implementation in
  `es-runtime-cli-common`, parameterised by a single baseline — so the two
  cannot drift on what `--allow-net=…` means, only on where a flagless line
  starts.

  `--allow-all` / `-A` is accepted here and restates the default, mirroring
  `--deny-all` on `esrun`. `--allow-<name>` still requires `--deny-all`, because
  against "everything granted" there is nothing for it to add.

  Three things keep the gap visible rather than letting a permissive dev loop
  hide what a deployment will need: `esdev --trace-permissions` prints the
  `esrun` line a run needs; `esdev start` spawns its child under `esdev.json`'s
  `permissions`, so a real project's dev loop already runs under the production
  grant; and `esdev create` scaffolds that block from the first commit.

- **`--trace-permissions` prints a shorter line.** It used to emit
  `esrun --deny-all --allow-read --allow-net app.js`; the `--deny-all` is now a
  no-op, so it emits `esrun --allow-read --allow-net app.js`. This is the
  command that closes the gap between the two defaults — run it once and paste
  the result into your deploy line.

- **`esdev.json`'s `permissions` is validated as the deploy grant.** A block with
  only grants — `{"allow": {"read": ["./dist"]}}` — is now valid on its own,
  where it used to need `"deny": ["all"]` beside it. The flags handed to the
  `esdev start` child carry an explicit `--deny-all`/`--allow-all`, so the block
  means the same thing whichever binary reads it. `"deny": ["all"]` still
  parses; the React template drops it. A block that subtracts writes
  `"allow": {"all": true}` alongside its denials.

### Fixed

- **A guest build now runs the same owned passes as `esdev build`.**
  `runtime:build` installed only the guest's plugins, while the `build`
  subcommand also installs this project's CSS Modules pass — so the same project
  produced a scoped `styles.button` from one path and an unscoped one from the
  other, and markup that did not match its own stylesheet. Both paths now build
  through one plugin list.

- **The React template's Content-Security-Policy blocked its own live reload.**
  `esdev start` injects its reload client as an inline script that cannot carry
  a per-response nonce, and serves its endpoint from another loopback port — so
  `script-src 'nonce-…'` and `connect-src 'self'` blocked the two halves and the
  page silently never updated on a save. A development build now allows inline
  script (dropping the nonce, since a policy that carries one *ignores*
  `'unsafe-inline'`) and admits loopback on any port. The production policy is
  unchanged.

  The dev/production flag is now passed into `src/http/` rather than read there.
  Those modules have no imports on purpose, so `esdev test` can run them
  unbundled — where `process.env.NODE_ENV` has been replaced by nothing and
  there is no `process` global to fall back on.

- **The React template's error page rendered unstyled.** A route's
  `ErrorBoundary` replaces its own route's element, and the boundary sits on the
  layout — so a 404 arrived with no shell, no masthead, and no way back except
  the browser's Back button.

- Markdown backticks in the template's sample posts rendered literally, there
  being no markdown renderer; a `<Link>` styled as a button was underlined.

## [0.1.0] - 2026-08-13

### Added

- **`esdev create`** (DECISIONS D64) — a project that already works.

  ```sh
  esdev create my-app
  esdev create --list
  ```

  Everything the four increments above built is only reachable if somebody can
  get to a working project without assembling one: an `esdev.json` with the
  right targets, an `index.html` whose script tag names the entry, a server that
  reads its template from beside itself, and a permission line that is narrow
  from the first run rather than widened to `--allow-all` on the way to a demo.

  **The templates are baked into the binary** (a build script walks
  `crates/dev-cli/templates`), so `create` works offline and always writes a
  project the `esdev` that wrote it can build. A scaffolder that downloaded
  could hand you something this binary was never tested against — and remote
  module imports are a stated non-goal, which a template download would be by
  another name. What a *running* template leaves behind — `node_modules`,
  `dist`, a lockfile — is skipped, and a test asserts it: embedding an installed
  `node_modules` would put tens of megabytes of somebody else's code in this
  binary and nothing about the build would complain.

  **It writes files and stops.** No install (there is no lockfile yet to say
  which package manager this project uses, and guessing wrong leaves the wrong
  one behind), no `git init`, no prompts — every other command here is a flag
  grammar that works in a script.

  **It never overwrites.** `esdev build --lib` empties its output because the
  build owns it; this owns nothing, so a non-empty directory is refused, and
  `--force` writes *among* what is there while still leaving every existing file
  alone.

  The project's name comes from the directory, into `package.json` and the
  document's `<title>`, lowercased and hyphenated if npm would reject it.
  `_gitignore` is written as `.gitignore` — as itself, it would apply to the
  template in this repository and untrack the file it means to ship.

- **A React template** (DECISIONS D63) — server-rendered, hydrated, and
  prerenderable to static HTML, from one project and one build.

  ```sh
  esdev build       # → dist/
  ```

  | Deploy | Run |
  | --- | --- |
  | `dist/` | `esrun --deny-all --allow-read=./dist --allow-listen=8080 dist/server.js` |
  | `dist/static/` | any static host |
  | `dist/index.html` + `dist/assets/` | any static host, with a fallback to `index.html` |

  **One build produces all three, and one client bundle serves them**, because
  the browser entry hydrates what it finds and renders from scratch when it
  finds an empty root. The prerendered pages go to `dist/static/` rather than
  over `dist/index.html`, which is the *template* the server splices into — a
  rendered page is not a template.

  What makes it one project rather than three is that the render is one
  function: `src/render.tsx` streams a document, and the server and the
  prerender step both call it. A page cannot come out one way live and another
  way static.

  It is deliberately short of a framework. Navigation is `<a href>`; the route
  table is three fields (path, component, loader) and no dependency; the loader
  runs on the server and its result reaches the browser in a `<script>` that
  escapes `<`, so a string in your data cannot close the tag it is inside.

  Two constraints it works within rather than around, both `esdev`'s and both
  documented in its README: there is **no CSS pipeline**, so `styles.css` is
  linked from `index.html` and copied by the build; and **component tests
  cannot run**, because `esdev test` runs each file unbundled and React ships
  CommonJS — the one test it ships covers a module that imports nothing.

- **`esdev start`** (DECISIONS D62) — the dev loop: build, run, rebuild, reload.

  ```sh
  esdev start
  ```

  It is `esdev build` on a loop. **A dev build differs from a release build in
  two ways** — `process.env.NODE_ENV` is `"development"`, and nothing is
  content-hashed — and in nothing else: a dev and a prod that disagree about how
  a module resolves is the failure this toolchain is arranged to prevent.

  | | |
  | --- | --- |
  | `"start": { "run": "server" }` | The named target's output **is** your server. esdev runs it as a child under the config's `permissions`, and restarts it with a `SIGTERM` — so a request in flight when you save is answered rather than dropped |
  | No `run` target | A static site or SPA has no server to be that, so esdev serves the output directory: files, an `index.html` fallback for client-side routes, nothing else |
  | A failed build | **Changes nothing.** The rebuild happens *before* anything is stopped, so the server you were about to fix it on keeps answering — during the build as well as after a failed one |
  | Reload | A few lines injected into each built document, opening an `EventSource` against esdev. Sent *after* the restart: a page told to reload while the server is coming back gets a connection refused and stays blank |

  The reload endpoint is **esdev's**, not the application's, so no template ships
  dev-only code and the file you edit is never written to. Server-sent events
  rather than a WebSocket: one direction, one word, and `EventSource` reconnects
  by specification — which matters, because the thing it is connected to is a
  build tool the developer will restart.

  It is a full page load, not hot module replacement: nothing is preserved.

  **Two fixes fell out of building it**, both of which applied to `esdev build`
  already:

  - A bundler failure printed its `Debug` representation —
    `BatchedBuildDiagnostic([BuildDiagnostic { kind: "PARSE_ERROR", … }])`. It
    now names the file and says what happened: `src/App.tsx: Unexpected token`.
  - The watcher matched its ignored directory names (`node_modules`, `dist`,
    `target`, …) **anywhere in the path**, so a project living in a directory
    called `target` — or a test fixture under `target/tmp` — had every one of its
    files ignored, and the symptom was a watcher that started, reported, and
    then reacted to nothing. They are matched below the watch root now.

- **An `index.html` target** (DECISIONS D61) — the document is the entry, and
  the tags in it name the build's inputs.

  ```json
  { "targets": { "web": { "entry": "index.html", "outdir": "dist" } } }
  ```

  ```html
  <!-- written -->                          <!-- built -->
  <link rel="stylesheet" href="./styles.css">
  <script type="module" src="./src/entry.client.tsx"></script>

  <link rel="stylesheet" href="/assets/styles-621d3b66.css">
  <script type="module" src="/assets/entry.client-fccaa347.js"></script>
  ```

  A server bundle's entry is a module, because the runtime starts at one. The
  browser does not — it starts at a **document**, and the module is something
  that document happens to reference. Naming the client entry in a config file
  *and* its built URL in the HTML meant two places that were one rename apart
  from disagreeing, with nothing to catch it.

  | | |
  | --- | --- |
  | `<script type="module">` | An **entry**: it and everything it imports become one browser bundle |
  | Everything else relative | **Copied** — stylesheets, favicons, images, classic scripts |
  | Both | **Content-hashed** into `<outdir>/assets`, so a deployment can cache the whole directory immutably |
  | Everything else in the file | **Untouched, byte for byte** — the title, the meta tags, the Open Graph block, the inline analytics snippet |

  That last row is literally true, and it is why the parser is a tokenizer that
  reports **byte spans** (`html5gum`, MIT). The rewrite is a splice into the
  original text, so the doctype's casing, the author's single quotes, their
  `&mdash;` and their stray whitespace inside a tag are never read, let alone
  rewritten — only the attributes being changed move. A tree-based parser would
  print the whole document back and normalise all of it.

  It is also a real tokenizer rather than a search for `src=`, which matters
  more than it sounds: a URL inside a `<script>` string, a CSS comment, an HTML
  comment or a `<textarea>` is **text**, and a build that treated one as a
  reference would fail on a page that is perfectly correct.

  **A relative path is an input; anything else is a URL.** `/assets/vendor.js`,
  `https://…`, `//cdn…` and `data:` are left exactly as written — one line of
  rule, and the escape hatch for anything the build should keep out of. A
  relative path that names nothing stops the build, because the alternative is
  finding it in a browser.

- **`esdev.json`** (DECISIONS D60) — what a project builds, in a file rather
  than on a command line.

  ```sh
  esdev build                      # every target
  esdev build --target=browser     # one of them
  ```

  ```json
  {
    "targets": {
      "server":    { "entry": "src/server.ts", "out": "dist/server.js",
                     "assets": ["index.html", "public"] },
      "browser":   { "entry": "src/entry.client.tsx", "outdir": "dist/client",
                     "platform": "browser" },
      "prerender": { "entry": "src/prerender.ts", "out": "dist/prerender.js",
                     "then": "run" }
    }
  }
  ```

  Every knob in this tool has been a flag until now, and for a *run* that is
  right: a flag is typed by a person, in view, once. A **build** is not that. An
  application that renders on the server and hydrates in the browser is two
  bundles from two entries with two shapes of output, and the site it prerenders
  is a third that has to *run* — none of which one command line can say. Spelled
  out in `package.json` scripts instead, it gets spelled out twice, once for the
  dev loop and once for the release, where the two quietly drift.

  Four keys carry the difference between the stacks, and nothing else changed:

  | | |
  | --- | --- |
  | `out` **vs** `outdir` | One file, or a directory. A dynamic `import()` emits a chunk beside its entry, and a build whose whole output is one named file has nowhere to put a second one |
  | `platform` | `server` (default) or `browser` — see below |
  | `assets` | Copied into the output: a file by name, a **directory by its contents**, so `public/styles.css` is served at `/styles.css` with nothing rewriting an href |
  | `then: "run"` | Execute the output once it is built. How a static site is generated without `esdev` knowing what one is: the bundle runs, and what it writes is the build's real output |

  A backend is one `out`; a frontend is a browser `outdir` plus a prerender
  target; a fullstack app is both. `esdev build <entry>` still ignores the file
  entirely, and a flag still beats it — `--minify` takes a release build of a
  project whose day to day is unminified.

  **`platform: "browser"` fixes a bug that was silent.** An application build
  asserts the `worker` condition, which is how `react-dom/server` hands over its
  Web Streams implementation rather than its `node:stream` one — correct for a
  server bundle and wrong for a client one, which wants `browser`. Conditions
  match in the order the *package author* wrote them (D40), so asserting
  `worker` at all was enough to win, and the failure was not at build time but
  in somebody's browser. A browser target now asserts `browser` instead, and
  gets rolldown's browser platform with it — the `browser` field, which predates
  `exports` and is still how a good deal of the registry redirects away from
  `node:` builtins.

  **The file is data, not a program.** Vite and Next both take an executable
  config and both are right to: theirs carry plugins, and a plugin is a
  function. `esdev` has no plugin API, no resolver hooks and no transform
  pipeline to configure, so a `.ts` config here would be a program whose entire
  content is data — and this file carries `permissions`, which means executing
  it to learn what a run may do would mean running guest code before that has
  been decided. The key names are chosen so a future `esdev.config.ts` can
  export the same shape and leave every existing `esdev.json` valid.

  **`esrun` does not read it, and will not.** A production binary that picks up
  a checked-in file granting itself capabilities is precisely what the
  capability model exists to prevent. `permissions` in the file shapes the child
  a developer's machine runs, which is how you develop *under* production's
  grants without being able to ship them by accident.

  A mistyped key is an error naming the key it was nearly (`outDir` →
  `outdir`), never a setting that silently does nothing; and `permissions` is
  translated into the flags it stands for and handed to the parser `esrun` uses,
  so the file cannot mean anything a command line could not.

- **`esdev build --lib`** (DECISIONS D59) — a source tree in, a publishable
  library out.

  ```sh
  esdev build --lib src            # src/** → dist/**.js + dist/**.d.ts
  ```

  `esdev build` was written for the artifact that gets *deployed*, and all four
  of its settings are right for that and wrong for a **library** — which is why
  this repository's own two drivers were built by `tsc` rather than by their own
  runtime's tool. A library is not the end of the line; it is an input to
  somebody else's build, so each of those decisions belongs to *them*:

  | | |
  | --- | --- |
  | Dependencies stay **external** | Inlining one publishes a private copy nobody can dedupe, override or patch |
  | Module structure **preserved** | A subpath in your `exports` map has to *be* a file |
  | **Nothing** defined, **no** condition asserted | `NODE_ENV` and `worker` freeze the consumer's environment into your package |
  | **`.d.ts`** beside each module | A library is a typed contract |

  **It takes a directory, not an entry, and that is the decision the rest
  follows from.** A bundle has one root — that is what makes it one file. A
  library has none: which modules a consumer may import is decided by your
  `exports` map, long after this build ran. So every module under the directory
  is built, and built as an *entry*, which is what keeps an export that no
  current caller uses. Found the hard way — an earlier version that took an
  entry shook `BLOCKING_COMMANDS` out of the Redis driver because only a test
  imported it, and the failure was a `SyntaxError` in the consumer rather than
  anything the build said. **An export nothing uses yet is not dead code in a
  library; it is the API.** Whatever really is dead, the consumer's build
  removes.

  Skipped: `*.test.*` and `.d.ts` files. `--out=<dir>` moves the tree
  (default `dist`).

  **The output directory is emptied first**, because the build owns it. Delete a
  module from `src` and without this its `.js` and `.d.ts` stay in `dist` for
  ever — and `"files": ["dist"]` puts them in the tarball, where a consumer can
  still import a module the library no longer has. An `--out` that holds your
  source or your project is *refused* rather than emptied: it is a path off a
  command line, and `--out=src` is one keystroke from `--out=dist`. An
  application build does not clean — its `--out` names one file, in a directory
  that may hold other builds and other people's files.

- **Declarations, derived from the source's own annotations** (DECISIONS D59) —
  emitted by `--lib` unless `--no-types` says otherwise.

  They are read off what the source *says*, never worked out by a checker: the
  same "erased, never checked" contract type-stripping has, and microseconds per
  file rather than a typechecker's pass. The price is TypeScript's
  `isolatedDeclarations` rule — an exported signature has to state its type:

  ```ts
  export const driver = defineDriver({ … });                     // ✗
  export const driver: Driver<Conn, Opts> = defineDriver({ … });  // ✓
  ```

  One that does not **fails the build with the whole list**, rather than getting
  a declaration that had to be guessed — a wrong `.d.ts` is worse than none,
  because it is believed. Measured before it was committed to: this
  repository's two drivers needed **nine annotations between them**.

  Verified against those packages, which is the only validation of a library
  builder worth having: both build into the tree `tsc` produced, their
  declarations typecheck under `tsc --noEmit` and a consumer resolves through
  them, and their unit suites pass unchanged under `esrun` against the
  `esdev`-built `dist/`.

- **`esdev build --lib --dts-bundle`** (DECISIONS D59) — every declaration in
  the library, linked into one `.d.ts`.

  ```sh
  esdev build --lib src --dts-bundle    # → dist/index.d.ts
  ```

  A package whose `exports` map has a single entry wants one declaration file,
  not a mirror of a source layout nobody outside the package should have to know
  about. **Nothing off the shelf does this**: `tsc` emits one `.d.ts` per source
  file and has no declaration-bundling mode (`--outFile` is a legacy
  `module: none/amd/system` feature), and rolldown's Rust crates have no `.d.ts`
  support at all — `rolldown-plugin-dts`, which its ecosystem uses, is an npm
  package rather than a crate. So the linker is written here, over the `oxc`
  parser and semantic analysis already in the tree.

  What it does, and each is a property a test pins against real output:

  | | |
  | --- | --- |
  | **Reachable, not everything** | Everything the entry's exports name, transitively |
  | **Inlined but not exported** | A type reachable only *through* a public one is present without widening your surface |
  | **Collisions renamed** | Two modules with `Options` become `Options` and `Options$1`, every site rewritten |
  | **Cycles followed** | `Tree` ↔ `Leaf` is ordinary in a type graph |
  | **Dependencies stay imports** | The same line `--lib` draws for JavaScript, and `import type` is kept |
  | **JSDoc byte for byte** | It is what an editor shows on hover |

  **Declarations travel as text, not as an AST.** Each one carries the bytes
  that produced it plus the byte ranges where a module-scope name appears, so a
  rename is a splice. That is what keeps JSDoc exactly as written — reprinting
  an AST reflows it — and it means no parser arena has to outlive the module it
  came from.

  **A rename is only sound if every site was found**, so the sites come from
  semantic analysis rather than a walk over the type syntax: `extends`, a mapped
  type's `keyof`, both branches of a conditional type, a default type argument.
  A hand-written walk would silently miss the corner nobody enumerated.

  A construct that cannot be linked into one file — a namespace import,
  `export =`, a module augmentation — **stops the build and names itself**,
  rather than being dropped into output that looks fine. A `.d.ts` is believed:
  nothing runs it and no test covers it. Build without `--dts-bundle` and the
  construct stands as written.

  Keep the per-module declarations if your `exports` map has subpaths —
  `@you/pkg/pool` has to find a real `pool.d.ts`.

  Verified against the two drivers again: `@opentf/esrun-postgres`'s 8 modules
  link into one 253-line declaration and `@opentf/esrun-redis`'s 14 into one of
  1269, both typecheck under `tsc --noEmit`, and consumers resolve through them
  — including the three names redis had to rename, whose public spellings the
  export block restores (`MessageContext$1 as MessageContext`). A deliberately
  wrong consumer is still rejected, which is what says the renames kept their
  identities rather than merely compiling.

## [0.1.0] - 2026-08-12

The first release. Everything below landed as one increment per feature
(DECISIONS D59) and is published together.

### Added

- **`esdev`, a second binary for local development** (DECISIONS D59). `esrun` is
  the production server runtime and stays narrow on purpose — no inspector port,
  no file watcher, no test discovery, nothing that could weaken the capability
  model it exists to enforce. That narrowness bills the developer's inner loop,
  and `esdev` is the binary that pays it.

  ```sh
  cargo build-dev          # → target/release/esdev
  esdev app.mjs            # the same run esrun gives, same flags, same grants
  ```

  This first increment is the foundation rather than the features: `esdev` runs
  a module or an inline snippet with `esrun`'s entire flag vocabulary, and does
  it by *sharing* the code rather than copying it. Watching, the inspector, the
  REPL, `test` and `build` land on top of this.

  **A program cannot behave differently under the two.** Everything that decides
  how a run behaves — the baked prelude snapshot, the D38 permission grammar,
  the provider wiring, the drive loop, graceful shutdown, the error block — moved
  into a new internal crate, `es-runtime-cli-common`, which both binaries sit on.
  Neither has a second copy of any of it, so they cannot drift; the dependency
  order is now `… → default-providers → cli-common → {runtime-cli, dev-cli}`.
  The snapshot is built once there too, so a second binary costs no second V8
  snapshot build.

  `esrun` is unchanged — same flags, same messages, same behaviour, verified by
  its existing 290-test suite passing untouched. `esdev` is **not** a deployment
  target and its `--help` says so; it is deliberately absent from the release
  manifest, so nothing new is published yet.

- **TypeScript and JSX in `esdev`** (DECISIONS D59) — `.ts`, `.tsx`, `.mts`,
  `.cts` and `.jsx` are stripped to JavaScript as they load, via
  `oxc_transformer`.

  ```sh
  esdev app.ts        # types erased on the way in
  esdev app.tsx       # …and JSX compiled
  ```

  It applies to the **entry file and everything it imports** — two different
  code paths, since the entry is read directly and imports come through the
  loader, and a transform wired into only one of them works on a hello-world
  and fails on every real program.

  **Types are erased, never checked**, the same contract Node's
  `--experimental-strip-types` and Bun have: a type error is your editor's job
  and `tsc --noEmit`'s, not something to put on the critical path of every run.

  **Import specifiers are left exactly as written.** `import './app.ts'` names
  the file that exists; there is no extension guessing, and `./app.js` does not
  resolve to `app.ts`. Resolution stays the loader's contract (D21/D40) — a
  transform that widened it would make `esdev` resolve differently from
  `esrun`, which is the one thing these two binaries must never do.

  JSX targets the automatic runtime, `react/jsx-runtime` by default, redirected
  per file with `/** @jsxImportSource remix/ui */`. A `.js` file is passed
  through untouched rather than reprinted, so its stack traces keep their own
  line numbers.

  **`esrun` is unaffected and still refuses the same file** — a test asserts
  exactly that, because the moment it passes, TypeScript has leaked into the
  production binary. `oxc` is a dependency of `dev-cli` alone.

- **`esdev build`** (DECISIONS D59) — a server entry and its dependencies, as
  one ES module, via `rolldown`.

  ```sh
  esdev build server.mjs                 # → dist/server.js
  esdev build src/app.ts --out=dist/app.js --minify
  ```

  **This is what makes the npm ecosystem reachable without weakening `esrun`.**
  The runtime loads ES modules only (D22) and much of the registry — React
  among it — still ships CommonJS. The conversion happens here, at build time,
  on the developer's machine; what `esrun` receives is ordinary ESM and the
  non-goal holds completely.

  **And it shortens the production command line.** An unbundled program needs
  `--allow-imports`, because the loader must walk `node_modules` at runtime. A
  bundle has no imports left to resolve:

  ```sh
  esrun --deny-all --allow-imports --allow-listen=8080 app.js    # unbundled
  esrun --deny-all --allow-listen=8080 dist/app.js               # bundled
  ```

  Four settings are why this is a command rather than a note telling you which
  flags to pass, and each fails *silently* when wrong: `runtime:*` stays
  **external** (it is served by the runtime and has no file behind it —
  inlining it yields a bundle that dies on its first import); output is
  **ESM**; `process.env.NODE_ENV` is **defined** to `"production"` (packages
  branch on it before doing anything, and this runtime has no `process`
  global); and the **`worker` condition** is asserted, which is how a package
  hands over its Web-API build instead of its `node:` one.

  `--conditions=<list>` adds to the defaults rather than replacing them, and
  `--define=<name>=<value>` overrides or extends the replacements. The runtime's
  own condition set stays standards-only (D40) — a condition changes which code
  runs, so it is chosen in a build you ran on purpose, not by a server resolving
  imports under load.

  Verified end-to-end: React 19 streaming SSR behind Hono, from CommonJS npm
  packages, bundled and then served by `esrun --deny-all --allow-listen` — one
  capability, and none of the four settings passed by hand.

- **`esdev --watch`** (DECISIONS D59) — rerun the program when its source
  changes.

  ```sh
  esdev --watch --deny-all --allow-listen=8080 server.mjs
  ```

  **A restart drains rather than drops.** The program runs in a child process
  and a restart is a `SIGTERM` — the same graceful stop production gets, so the
  Phase 14 shutdown path stops accepting, answers the requests already in
  flight, and only then exits. Saving a file while a request is open does not
  kill that request; verified with a two-second handler edited mid-request,
  which still returned its response. `--shutdown-grace` bounds the wait, after
  which the process is killed.

  A child process rather than an in-place teardown, because a fresh process
  cannot carry anything forward — no leaked socket, no wedged isolate poisoning
  the next run — and the prelude snapshot makes starting one cheap.

  Watched: the project root (nearest `package.json`, the same root the loader
  detects) or the entry's directory, minus `node_modules`, `.git`, `dist`,
  `target` and `.cache`, and only for source extensions. A program that exits
  leaves the watcher up, waiting for the next change.

- **`esdev test`** (DECISIONS D59) — find the test files, run each in its own
  process, report.

  ```sh
  esdev test              # every *.test.{js,mjs,ts,tsx,jsx}
  esdev test math         # ...whose path contains "math"
  esdev test --file=x.test.ts
  ```

  **One process per file.** A file that wedges, exhausts its heap or calls
  `exit()` cannot decide the fate of the others, and a global one file leaves
  behind is not visible to the next.

  **The test file is the entry**, not something a generated driver imports —
  which is not a stylistic choice: module resolution is jailed to the project
  root detected from the entry's own directory (D25), so a driver in a temp
  directory could not import a test file in the project at all. The harness is
  prepended to the file's own source through the same `SourceTransform` seam
  that strips TypeScript, so the file keeps its path, its jail, its relative
  imports and its `.ts` handling.

  The globals are the ones this repository's own conformance suite uses —
  `test`, `assert`, `assertEquals`, `assertThrows`, `assertRejects` — so a
  developer reading the runtime's tests and writing their own does not learn two
  vocabularies. `test` accepts async functions; failures are collected rather
  than aborting the file.

  The harness is injected as a **single line**, so a failing assertion still
  names the line the developer wrote rather than one thirty lines further down.

- **`esdev --inspect`** (DECISIONS D59) — a debugger, over the Chrome DevTools
  Protocol, in the binary that is not a deployment target.

  ```sh
  ES_RUNTIME_INSPECTOR=1 cargo build-dev     # the build that has one

  esdev --inspect app.ts            # 127.0.0.1:9229; attach and set breakpoints
  esdev --inspect-brk app.ts        # ...and stop before the first statement
  esdev --inspect=9300 app.ts       # a port, an address, or both
  ```

  Chrome's `chrome://inspect`, VS Code and any other CDP client attach to it:
  the `/json/version` and `/json/list` endpoints describe the target, sources
  arrive with their real `file:` URLs so breakpoints set by URL land in the file
  you are looking at, and a paused program is genuinely stopped — locals read
  through `Debugger.evaluateOnCallFrame`, nothing else running meanwhile.

  **Off unless the build asked for it, and `esrun` cannot have it at all.** An
  inspector port is a total bypass of the capability model: attach and you own
  the isolate, whatever `--deny-all` said. So the V8 half is compiled only when
  `ES_RUNTIME_INSPECTOR=1` is set for the build — an environment variable read
  by a build script rather than a Cargo feature, because Cargo unifies features
  across everything built in one invocation and a feature declared by `esdev`
  would also be on in the `esrun` beside it. `esrun`'s build script now *refuses
  to build* while that variable is set, so one invocation can never produce
  both. A build without it accepts `--inspect` and fails with the line telling
  you how to get one, rather than listening on nothing.

  The endpoint binds loopback by default and warns — rather than refuses — when
  told to bind elsewhere, and `esdev --watch --inspect` reclaims its port across
  a restart. **Known limitation:** `console.log` still goes to the terminal, not
  the debugger's console pane.

- **`esdev --trace-permissions`** (DECISIONS D59) — run the program once, and be
  told the `esrun` line to deploy it with.

  ```sh
  esdev --trace-permissions app.mjs
  ```
  ```text
  esdev: the permissions this run used

    read      fs_read
    imports   import
    net       fetch
    env       process_env

    esrun --deny-all --allow-read --allow-imports --allow-net --allow-env app.mjs
  ```

  This is the gap D59 was written about: `esrun` grants everything by default and
  can be narrowed to nothing, but **nothing helped a developer arrive at the
  right flags**, so in practice they shipped the default.

  What it records is the capability check itself, at op dispatch — the only place
  that knows what a program *reached for* rather than what it was handed. So it
  reports the ones it asked for and **was refused** too, which are listed and
  deliberately left out of the line: whether a refusal was correct is not a
  trace's call. Workers are traced into the same report, on their own threads and
  their own isolates — their grants are set at the spawn, which is where they are
  hardest to get right. The report is printed however the run ended, including
  the `process.exit()` and `^C` paths, which for a server is every run.

  Scopes are not traced: the line grants each capability unnarrowed, and narrowing
  it (`--allow-read=./data`) is still yours. The hook is an `Option` read once per
  dispatch and `None` in every `esrun` run.

- **`esdev --install-types`** (DECISIONS D59) — the `runtime:*` type
  definitions, in your editor.

  ```sh
  esdev --install-types
  ```

  It adds [`@opentf/esrun-types`](https://www.npmjs.com/package/@opentf/esrun-types)
  as a dev dependency — with the package manager your lockfile names (bun, pnpm,
  yarn, npm) — and registers it in `compilerOptions.types`, creating a
  `tsconfig.json` if there is none or merging into an existing one without
  touching your other settings. A JSONC config (comments, trailing commas) is
  left untouched with the lines to add printed instead, because re-emitting it
  from a JSON value would silently delete the comments.

  This was `esrun types --install`, and it is rewritten rather than moved: the
  definitions are published to npm, so nothing fabricates a package under
  `node_modules/@opentf/esrun` any more and no binary carries a copy of them.
  A command whose entire effect is to write into `node_modules` and rewrite a
  `tsconfig.json` is development tooling; `esrun` is the binary that should have
  none.
