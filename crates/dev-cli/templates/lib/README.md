# {{name}}

A publishable TypeScript package. **No dependencies, no bundler config.**

```sh
npm install       # nothing to install, but it writes the lockfile
npm test
npm run build     # → dist/, with .d.ts beside each module
```

## What is here

| | |
| --- | --- |
| `src/index.ts` | **The public surface.** Everything a consumer can reach is re-exported here |
| `src/result.ts` | An example module |
| `src/retry.ts` | A second one, with behaviour worth testing |
| `package.json` | `exports` points at `dist/`, `files` publishes only that |

## A library is not an application

`esdev build --lib` is a different build, and every difference is deliberate:

| | |
| --- | --- |
| **Module structure is kept** | One file in, one file out. That is what makes a subpath `exports` map possible, keeps a stack trace pointing at a module, and lets a test import an internal file the package does not export |
| **Dependencies stay external** | Inlining one ships a private copy your consumer cannot dedupe, override or patch |
| **Nothing is defined, no condition asserted** | `NODE_ENV` and `worker` are your *consumer's* build's call. Baking them in freezes their environment into your package |
| **`.d.ts` travels with the `.js`** | A library is a typed contract |

## The declarations come from your annotations

They are **derived, never inferred** — which is what makes emitting them fast
and exact. The cost is one rule:

```ts
export function ok<T>(value: T): Ok<T> { … }   // ✅ annotated
export function ok<T>(value: T) { … }          // ❌ the build says so
```

`tsconfig.json` sets `isolatedDeclarations`, so your editor tells you before the
build does.

## The public surface is one file

`package.json`'s `exports` names `dist/index.js` and nothing else. A module not
re-exported from `src/index.ts` is internal: it can be renamed or deleted
without a major version, because nothing could have imported it.

Add a second entry point deliberately:

```json
"exports": {
  ".":       { "types": "./dist/index.d.ts",  "default": "./dist/index.js" },
  "./retry": { "types": "./dist/retry.d.ts",  "default": "./dist/retry.js" }
}
```

That works *because* module structure is preserved — `./retry` has to be a real
file for a consumer to import it.

## Publishing

```sh
npm version minor
npm publish
```

`prepublishOnly` runs the build, so what is published is always current. `files`
is `["dist"]`, so `src/` and the tests stay out of the tarball — check with
`npm pack --dry-run`.

## Tests

```sh
npm test
```

`esdev test` runs each file directly. There is no framework, no config, and no
build step between the source and the test — a test imports the module beside
it, including ones the package does not export.
