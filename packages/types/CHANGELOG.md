# Changelog for `@opentf/esrun-types`

All notable changes to **`@opentf/esrun-types`**, the TypeScript definitions for
ES Runtime's `runtime:` standard modules, are recorded here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

This package describes a surface it does not implement, so its releases track
the runtime's: a `runtime:` module that gains, loses or changes an export shows
up here in the same release that ships it. The project is pre-`1.0` and that
namespace is unstable until the API freeze (SPEC §14), so a type can change in a
minor release. See the root [CHANGELOG.md](../../CHANGELOG.md) for the runtime
itself.

## [Unreleased]

### Added

- **`runtime:test`'s `it`, `suite`, and the table forms** — `Each`, `TestFn`,
  and `.todo`/`.skipIf`/`.runIf`/`.each` on both `test` and `describe`. A row
  written `as const` is a tuple and its body's parameters are checked against
  it; a plain array row infers as an array, which is TypeScript's rule rather
  than a looseness here, and the type test pins both.

- **`runtime:test`'s `expect`, `mock` and `clock`** — `Matchers`,
  `AwaitedMatchers` and `Assertion` for the matcher vocabulary (including
  `.not`, `.resolves`/`.rejects` and the asymmetric factories on `expect`
  itself), and `Mock`/`MockRecord` for a recording function. Without these a
  `.ts` test file referenced undeclared names and `tsc --noEmit` failed on a
  suite that ran perfectly — the failure `runtime:test` was made a module to
  avoid.

- **`runtime:test`'s `describe`**, and `.skip`/`.only` on it and on `test`.
  `test` becomes a callable object rather than a function declaration so the two
  can hang off it.

- **`runtime:fs`'s `symlink(target, path, options?)`**, with `SymlinkOptions`.

### Fixed

- **`PooledConnection` declared that it implements `Connection` and did not.**
  It was missing `subscribe`, `unsubscribe`, `subscribed`, `subscriptions`,
  `usable` and `reusable` — all six of which the implementation has — so the
  class was a `TS2420` for anyone who type-checked this package's declarations
  rather than skipping them. `subscribe` and `unsubscribe` return `Promise<never>`,
  because a pool refuses both: a subscription needs a connection of its own.

  Found by giving the package a `tsconfig.json` with `skipLibCheck: false` and
  a `test/` of `@ts-expect-error` cases, now run by `tsr typecheck`. These
  declarations describe a surface they do not implement, so nothing else could
  catch them being wrong — the runtime's own suite proves the code works and
  would go on passing while the types beside it said something else.

### Changed

- **`HookFilter` names `id`, `code` or both.** It was an interface with two
  optional keys, which made `filter: {}` — and, through structural typing, a
  bare `filter: /\.mdx$/` — legal to write and a catch-all to run. It is a
  union of the two one-key-required shapes now, so the editor refuses what the
  runtime refuses.

### Fixed

- **`EmittedFile` was declared twice** in `runtime-build.d.ts`, which is a
  duplicate-identifier error for anyone typechecking against it. One copy left.

## [0.3.0] - 2026-08-19

### Added

- **`runtime:workers` — the durable-worker surface**, in a declaration file of
  its own (`runtime-workers.d.ts`), referenced from `index.d.ts`.

  `DurableWorker` and `DurableRef<T>` are the pair that carries the design into
  the type system: `Cart.get("u_42")` is a *reference*, not an instance, so
  every method on it comes back as a promise whether the class wrote it `async`
  or not — which is what a call that crosses into the runtime actually is.

  With them: `DurableState` and `DurableContext`, the collection surface
  (`DurableCollection`, `DurableQuery`, `DurableSchema`, `DurableWhere`,
  `DurableTest`, `DurableField`, `DurableKeyRange`), the alarm surface
  (`DurableAlarm`, `AlarmScheduler`, `AlarmOptions`), `DurableWorkerInfo`,
  `DurableConfig`, `configure()`, and `DurableError` with its `DurableErrorCode`
  table. See the root [CHANGELOG.md](../../CHANGELOG.md) for the runtime side
  (DECISIONS D80–D82).

- **`runtime:process`: `stdout` and `stderr`.** A `StdStream` — `write(chunk)`
  for exactly those bytes with **no newline added**, plus `isTTY`, `columns` and
  `rows`. The size members are `number | undefined`, deliberately: a host that
  cannot answer says so rather than reporting a plausible 80, and the type is
  what makes a caller write `stdout.columns ?? 60`.

- **`runtime:test`: the lifecycle hooks.** `beforeAll`, `afterAll`, `beforeEach`
  and `afterEach`, and the `Hook` type they take.

- **`runtime:build`: what a failed build is.** `BuildError` and `BuildFailure` —
  `errors` is the whole batch, each with `message`, `id`, `plugin`, `kind`,
  `line`, `column` and `frame`. The nullable members are typed `| null` rather
  than optional, because a diagnostic that pointed at no place still carries the
  field.

- **`runtime:build`: the `bundle` hook**, with `BundledFile` — the discriminated
  union of `{ type: "chunk", … }` and `{ type: "asset", fileName }` a plugin is
  handed after the graph is split. It carries no `code`, and the type says so.

- **`runtime:build`: `facadeModuleId` on `OutputChunk`** — the module a chunk
  *is*, `string | null`.

- **`runtime:build`: `PluginJsx`**, the `jsx` a plugin declares alongside its
  hooks — what it needs the *compiler* to do, which no hook signature can
  express.

- **`runtime:build`: `PluginContext` gains `type` and `refresh`.** Both
  optional, because both are present only for the hook and the build that has
  them: `type` on `transform`, `refresh` only while the dev loop is running that
  target hot.

### Changed

- **`test()`'s documentation says tests run one at a time.** No signature moved.
  The old doc comment promised the opposite — *"tests are not queued: each one
  starts when `test()` is called"* — which is now false, and a doc comment that
  is false about ordering is worse than none: it is what a reader reaches for
  before writing a suite that shares a database.

## [0.2.0] - 2026-08-17

### Added

- **`import.meta.hot`**, the hot-replacement API `esdev start` provides:
  `accept` in its four forms, `signal`, `keep`, `dispose`, `data`, `decline` and
  `invalidate`.

  Not a `runtime:` module, and here for the same reason `runtime:build` is — the
  surface exists only under `esdev`, and a project written against it still has
  to typecheck. It is **optional** (`hot?`), because `esrun` injects nothing and
  a deployed build has no such property, which makes `if (import.meta.hot)` the
  shape that compiles for both.

### Changed

- **`runtime:build`'s options say what they do.** `conditions` are *appended* to
  the ones the platform already asserts (`worker` for `neutral`, `browser` for
  `browser`), `mainFields` *replaces* the default `["module", "main"]`, and
  `platform` decides which conditions those are. No type changed shape; what
  changed is that the ones that were easy to read backwards now say which way
  they go.

## [0.1.0] - 2026-08-15

### Added

- **Type definitions for the `runtime:` standard modules**, as ambient
  `declare module` blocks — add the package to `compilerOptions.types` (or
  reference it from one file) and the imports are typed:

  ```ts
  import { file, write } from "runtime:fs";
  ```

  Covered: `process`, `path`, `fs`, `db`, `net`, `http`, `websocket`,
  `serialization`, `hashing`, `wasi`, `system`, `build`, `test`, and `watch`,
  plus the few globals whose shape here differs from the standard libs. Web
  globals (`URL`, `Blob`, `ReadableStream`, `Response`, …) are not redeclared:
  esrun targets the WinterTC surface, so those come from your `lib`.

- **`runtime:build`** — the bundler's types, including the plugin contract:
  five hooks (`start`, `resolve`, `load`, `transform`, `end`), each an object
  carrying a `handler` with a declarative `filter`, and a context passed as the
  last argument rather than as `this` so an arrow function keeps it.

- **`runtime:test`** — the test API, imported rather than ambient, so nothing is
  declared globally that only exists under a test run.

- **`runtime:watch`**, and the definitions for the `runtime:` modules a binary
  adds on top of `esrun`'s.

- **`esdev --install-types` installs this package** and registers it in
  `compilerOptions.types`, creating a `tsconfig.json` if there is none. Nothing
  fabricates a package under `node_modules/@opentf/esrun` any more, and no
  binary carries a copy of the definitions.

### Fixed

- **`files` is a glob, not a list.** The hand-maintained list had drifted:
  `globals.d.ts` and `runtime-websocket.d.ts` were referenced by `index.d.ts`
  and not published, so an installed package could not resolve its own
  references. A glob cannot fall behind a new module.

[Unreleased]: https://github.com/Open-Tech-Foundation/ES-Runtime/commits/main/packages/types
