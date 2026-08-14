# {{name}}

TypeScript and the DOM. **No framework, no dependencies.**

```sh
npm install       # nothing to install, but it writes the lockfile
npm run dev       # http://localhost:5173
```

Swap `npm` for `bun`, `pnpm` or `yarn`; nothing here depends on which you use.

## What is here

| | |
| --- | --- |
| `index.html` | The document. Its `<script>` and `<link>` are the build's inputs |
| `src/main.ts` | The render loop, and the DOM |
| `src/items.ts` | The state's shape and the logic over it — **the part with tests** |
| `src/Counter.module.css` | Component styling, scoped to that file |
| `styles/app.css` | The baseline. `@import` works; esdev bundles it |

## The one pattern worth copying

```
event → update state → render()
```

State in one place, one function that turns it into DOM, and events that change
the state rather than the DOM. `render()` rebuilds from scratch every time,
which is not the fastest thing possible and is the right default: it is
*impossible* for the page to disagree with the state, because the page is the
state.

The alternative — a handler that edits the DOM *and* keeps a variable in step —
is where a frameworkless app usually starts going wrong, because nothing makes
the two agree and nothing tells you when they stop. Reach for something finer
only when a measurement says to.

## Logic and DOM are separate on purpose

`src/items.ts` imports nothing and touches no DOM, so `esdev test` can run it:

```sh
npm test
```

There is no DOM in the runtime, so anything reaching `document` cannot be tested
here. That is a good reason to keep the logic out of the rendering, which is
worth doing anyway.

## Building it

```sh
npm run build     # → dist/
```

Everything the document references is hashed into `dist/assets` and the document
is rewritten to point at it, so the whole directory caches immutably. Deploy
`dist/` to any static host.

`npm run build -- --minify` for a release.

## Styling

`styles/app.css` is linked from `index.html`; its `@import`s are resolved at
build time into one hashed file.

A file named `*.module.css` is scoped to itself — import it and you get the real
class names:

```ts
import styles from "./Counter.module.css";
element.className = styles.count;
```

`.count` becomes `.count_a1b2c3d4`, so two files can both declare `.count` and
neither wins.

## What this template is not

It has no router and no server. Every URL is `index.html`; there is nothing
rendering on a server and nothing to deploy but files.

If you want either, `esdev create --template=react` starts from a route table
and a server that renders it.
