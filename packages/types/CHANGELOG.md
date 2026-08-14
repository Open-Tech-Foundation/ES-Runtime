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
