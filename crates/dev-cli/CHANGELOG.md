# Changelog for `esdev`

All notable changes to **`esdev`**, the ES-Runtime's local development binary,
are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

`esdev` is versioned **separately from `esrun`** and from the Rust crates: it is
a tool rather than a contract, and its command line moves at its own pace. The
runtime it runs your program on is `esrun`'s — same prelude, same snapshot, same
providers, same capability enforcement — so what changes here is everything
*around* a run, never what the JS sees. The one deliberate difference is the
default grant: `esdev` starts from every capability and `esrun` from none
(DECISIONS D65), so that an inner loop needs no flags and a deployment states
what it may reach. See the root [CHANGELOG.md](../../CHANGELOG.md) for the
runtime itself.

`esdev` is **not a deployment target**: ship the artifact and run it under
`esrun`, which has no development surface to attack.

## [Unreleased]

### Changed

- **`esrun` now grants nothing by default; `esdev` still grants everything**
  (DECISIONS D65). This is the one place the two binaries differ, and it is
  deliberate: a dev loop that dies on an unnamed capability at every `--watch`
  save is the friction D59 put this binary here to absorb. The flags, scope
  lists, rules and error text are one implementation in
  `es-runtime-cli-common`, parameterised by a single baseline.

  `--allow-all` / `-A` is accepted here and restates the default, mirroring
  `--deny-all` on `esrun`. `--allow-<name>` still requires `--deny-all`, because
  against "everything granted" there is nothing for it to add.

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
