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

- `DurableErrorCode.Cycle` (`ERR_DURABLE_CYCLE`, DECISIONS D130).
- **Durable-worker shards** — `shards`, `module` and `permissions` on
  `DurableConfig`, and `DurableErrorCode.ShardLost`.
- **`toMatchScreenshot`** on `Matchers`, and `ScreenshotOptions`.
- **Benchmarks** — `bench` in `TestContext`, `Bench` (with `from`),
  `BenchTask`, `BenchOptions`, `BenchTaskOptions` (with `writeResult`),
  `BenchTimer`, `BenchResult` (with `warnings` and `stored`),
  `BenchResultData`, `BenchStatistics`, and the `toBeFasterThan`/`toBeSlowerThan`
  matchers.
- **`test.concurrent`, `test.sequential`, `describe.concurrent`,
  `describe.sequential`** and `concurrent` in `TestOptions`/`DescribeOptions`.
- **`expectTypeOf` and `assertType`**, with `ExpectTypeOf`, `TypeMismatch` and
  `TypeCheckFailed`.
- **Tags** — `tags` in `TestOptions`, `DescribeOptions`, the augmentable
  `TestTags` and `TagName`, and `matchesTags`.
- **`test.extend` and the test context** — `TestAPI<Fixtures>`, `TestContext`,
  `FixtureOptions`, `FixtureHelpers`; `TestBody`, `Register` and `TestFn` take
  the fixtures as a type parameter, and `beforeEach`/`afterEach` take the
  context (`EachHook`).
- **`unrefTimer` and `refTimer`** in `runtime:process`.
- **`inject`**, with the `ProvidedContext` interface to declare provided keys
  and the `GlobalSetupContext` a global setup's `setup` receives.
- **The new matchers and `expect` utilities** — `toHaveBeenCalledBefore`,
  `toHaveBeenCalledAfter`, `toHaveBeenCalledExactlyOnceWith`, the
  `toHaveResolved*` family, `toBeNullable`, `expect.arrayOf`,
  `expect.schemaMatching`, `expect.fail`, `expect.addEqualityTesters` and
  `expect.addSnapshotSerializer` — with the `StandardSchemaV1`,
  `EqualityTester`, `TesterContext` and `SnapshotSerializer` types, and
  `settledResults`, `contexts` and `invocationCallOrder` on `MockRecord`.
- **`clock.freeze(options)`** with the `FreezeOptions` and `Fakeable` types, and
  `clock.advanceToNextFrame` and `clock.runMicrotasks`.
- **`mock.when`** with the `When`, `WhenOptions` and `AnswerOptions` types, and
  the `toHaveBeenExhausted` matcher; **`mock.env`**; and, on `Mock`,
  `mockThrow`, `mockThrowOnce`, `withImplementation`, `getMockImplementation`
  and `Disposable`.
- **`mock.module` and `mock.importActual`** in `runtime:test`. A `mock.module`
  call with an async factory is typed to return `Promise<void>`.
- **`runtime:test`'s new utilities** — test options (`timeout`, `retry`) and
  `test.fails`; `expect.extend` with the `MatcherResult`, `MatcherContext`
  and `CustomMatcher` types, and an `AsymmetricMatchers` interface to augment;
  `expect.soft`, `expect.assertions`, `expect.hasAssertions`,
  `expect.unreachable`, `expect.poll`, `expect.closeTo` and `expect.not`;
  `onTestFinished`, `onTestFailed` and `waitFor`; `toSatisfy`, `toBeOneOf` and
  the DOM matchers.
- **`toMatchInlineSnapshot` and `toThrowErrorMatchingInlineSnapshot`**.
- **The `repeats` test option.**

### Changed

- `DurableConfig.dir` says a relative path is relative to the working
  directory, not the entry file (DECISIONS D129).

## [0.7.0] - 2026-09-23

### Added

- **`runtime:test`'s `toHaveBeenCalledOnce()` and accessor `spyOn`** —
  `toHaveBeenCalledOnce()` asserts a mock was called exactly once, and
  `spyOn(object, key, accessType)` with `"get"` or `"set"` watches one
  accessor of a property while preserving the other.

## [0.6.0] - 2026-09-20

### Fixed

- Fix `tsr lint:ci` on main: apply Biome's format/organize-imports fixes in the
  type tests, add the unused `dropped` binding to its `void` tuple, and assert
  the two `void`-returning calls through a `() => void` (a `void` variable
  annotation and `return <void>` are both lint errors).

## [0.5.0] - 2026-09-15

### Added

- **`runtime:context`** — `Context<T>`, `ContextOptions<T>` and `TaskInfo`, with
  `createContext`, `snapshot`, `bind`, `withTrace` and `currentTask`.
  `createContext<T>()` carries its value type through `.get()` and `.run()`, so
  a context declared to hold a `Request` cannot be `run()` with a string.
  `bind<F>` returns the same signature it was given rather than widening to
  `Function`, which is what lets a bound callback stay assignable to the
  parameter it came from.

- **`runtime:diagnostics`** — `SpanRecord`, `Filter`, `Batch`, `Subscription`,
  `SpanKind`, `SpanStatus`, `AttributeValue`, `HandleGroup`, `Histogram`,
  `Metrics`, `ProcessMetrics`, `GcMetrics` and `Span`, with `subscribe`,
  `inventory`, `metrics` and `span`.
  - `span` is **two overloads**: the handle form `span(name, options?)` returns
    a `Span`, and the scoped form `span(name, options, fn)` returns exactly what
    `fn` returned — including its `Promise`. Declared in that order so a callback
    argument selects the second rather than being rejected by the first.
  - `attributes` is `Readonly<{ [key: string]: AttributeValue }>`, so a nested
    object is a type error where it would otherwise have been silently dropped
    host-side.

- **`runtime:process` gains `memoryUsage()`, `cpuTime()` and `uptime()`**, with
  `MemoryUsage`. All three are per **agent**, which the doc comments say at each
  one because the same names are process-wide in Node and the difference does not
  show up in the signature. `PermissionName` gains `diagnostics` and
  `diagnostics-detail`, without which `permissions.has("diagnostics")` — a call
  the runtime answers — did not compile.

- **`runtime:http`'s `ServeOptions` gains `trustTraceHeaders`**.

- **Type tests for both new modules** — `test/runtime-context.types.ts` and
  `test/runtime-diagnostics.types.ts`, run by `tsr typecheck`. They pin the
  overload resolution above and the `@ts-expect-error` cases that matter: a
  nested attribute value, a callback taking arguments, and the fields this
  surface deliberately does not have (`record.origin`, `Metrics.poolSaturation`,
  a per-space GC breakdown, a resource on a `HandleGroup`).

### Fixed

- **`PermissionName`'s documentation had detached from it.** The comment
  listing what each capability buys sat above `MemoryUsage` rather than above
  the type it describes, so the editor showed nothing on hover.

## [0.4.0] - 2026-09-01

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
