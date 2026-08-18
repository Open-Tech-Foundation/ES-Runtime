# {{name}}

A publishable TypeScript package built with the
[ES Runtime](https://esrun.opentechf.org) toolchain. **Nothing it ships depends
on, and there is no bundler config.**

```sh
npm install       # TypeScript and the runtime's types, both dev-only
npm test
npm run build     # → dist/, with a .d.ts beside each module
```

## What is here

| | |
| --- | --- |
| `src/index.ts` | **The public surface.** Everything a consumer can reach is re-exported here |
| `src/greeting.ts` | **Start here.** The one module this package has |
| `package.json` | `exports` points at `dist/`, `files` publishes only that |

## Commands

| | |
| --- | --- |
| `npm test` | `esdev test` — every `*.test.ts` |
| `npm run build` | `esdev build --lib src` |
| `npm run typecheck` | `tsc --noEmit`. esdev erases types and never checks them |

## A library is not an application

`esdev build --lib` is a different build, and every difference is deliberate:

| | |
| --- | --- |
| **Module structure is kept** | One file in, one file out — what makes a subpath `exports` map possible and keeps a stack trace pointing at a module |
| **Dependencies stay external** | Inlining one ships a private copy your consumer cannot dedupe, override or patch |
| **Nothing is defined, no condition asserted** | `NODE_ENV` and `worker` are your *consumer's* build's call |
| **`.d.ts` travels with the `.js`** | A library is a typed contract |

## Annotate every export

The declarations are **derived, never inferred**, which is what makes emitting
them fast and exact. The cost is one rule: a function whose return type is left
to inference cannot be emitted, and the build says so rather than guessing.
`tsconfig.json` turns on `isolatedDeclarations`, so `npm run typecheck` reports
it before the build does.

## Docs

[esrun.opentechf.org/docs](https://esrun.opentechf.org/docs) ·
[API](https://esrun.opentechf.org/api) ·
[GitHub](https://github.com/Open-Tech-Foundation/ES-Runtime)

Part of the [Open Tech Foundation](https://github.com/Open-Tech-Foundation)
ecosystem.
