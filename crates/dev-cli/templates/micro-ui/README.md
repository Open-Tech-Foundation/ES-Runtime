# {{name}}

A framework-free TypeScript micro app using
[Micro-UI](https://github.com/Open-Tech-Foundation/Micro-UI), a small
custom-element and HTML-template library from the Open Tech Foundation.

```sh
npm install
npm run dev       # http://localhost:5173
```

Swap `npm` for `bun`, `pnpm` or `yarn`.

## What is here

| | |
| --- | --- |
| `index.html` | The document and browser entry point |
| `src/main.ts` | **Start here.** The `x-counter` component and its state |
| `styles/app.css` | Micro-UI's styles plus the page layout |

Micro-UI keeps the browser close to the platform: `define()` registers a custom
element, `html` renders a safe template, and `update()` redraws it after state
changes. The `dev: true` option also enables helpful diagnostics while building.

## Commands

| | |
| --- | --- |
| `npm run dev` | The dev server, rebuilding on save |
| `npm test` | `esdev test` — every `*.test.ts` |
| `npm run build` | → `dist/`, ready for a static host |
| `npm run typecheck` | `tsc --noEmit` |

Part of the [Open Tech Foundation](https://github.com/Open-Tech-Foundation)
ecosystem.
