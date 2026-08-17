# {{name}}

A React app on the ES Runtime with **no server of its own**. One project, and
two things you can build from it — pick per deploy, not per project:

| | Build | Deploy | What the visitor's first request gets |
| --- | --- | --- | --- |
| **Static (SSG)** | `npm run build` | `dist/static/` | The page, already rendered as HTML |
| **Single-page (SPA)** | `npm run build:spa` | `dist/` | A shell, filled in by the browser |

Both come out of the same routes, the same components and the same stylesheet.
Nothing in `src/` knows which one you chose.

```sh
npm install
npm run dev      # http://localhost:5173
```

Swap `npm` for `bun`, `pnpm` or `yarn` throughout; nothing here depends on which
you use.

> Need a server — sessions, a database, an API of your own, per-request
> rendering? That is the other half of this template:
> `esdev create my-app --template=react --mode=fullstack`.

## What is here

| | |
| --- | --- |
| `src/routes.tsx` | **The app in one file** — paths, loaders, components, page titles |
| `src/prerender.tsx` | Writes every route to `dist/static/`. This is the static build |
| `src/paths.ts` | Which routes get written. Everything else is rendered in the browser |
| `src/render.tsx` | One render, streamed. Used by the static build |
| `src/app/` | The components |
| `src/http/head.ts` | Titles and meta, and the escaping that makes them safe |
| `src/data/` | Stands in for your CMS, your content files, or an API you fetch |
| `styles/app.css` | The stylesheet. `@import` works; esdev bundles it |
| `index.html` | The document. Your meta tags, and the two markers a render fills |
| `esdev.json` | What this project builds |

## Choosing, per deploy

`npm run build` builds both targets: the browser bundle, and then
`src/prerender.tsx`, which renders every path `staticPaths()` returns and writes
it as `dist/static/<path>/index.html`. That is the SSG output — HTML a host can
answer with immediately, which is what a search engine and a slow phone both
want.

`npm run build:spa` builds only the browser bundle. `dist/index.html` is one
shell for every route, so the host needs a fallback to it for unknown paths.
Nothing is rendered ahead of time and nothing has to be rebuilt when the content
changes underneath it.

The switch is which script you run. Neither one edits a file, and you can run
both from the same commit.

**Which routes are prerendered** is `staticPaths()` in `src/paths.ts`. It is an
ordinary async function, so a route backed by content can enumerate it:

```tsx
export async function staticPaths(): Promise<string[]> {
  return ["/", "/posts", ...(await posts()).map((post) => `/posts/${post.slug}`)];
}
```

Leave a route out and it still works — it is simply rendered in the browser
instead, on a static build that has an `index.html` fallback. So a mostly-static
site with a couple of client-only routes is this same project, with a shorter
list.

## Routing

`src/routes.tsx` is a [react-router](https://reactrouter.com) route table in
data mode. A route may carry:

- **`loader`** — runs before the component, on whichever side is rendering. On a
  prerendered page its result is baked into the document, so the hydrating page
  does not fetch what the build already had.
- **`Component`** — `useLoaderData()` gives it that result, typed per route.
- **`ErrorBoundary`** — what renders when a loader or a component below throws.
  A URL matching nothing arrives here too, as a 404.
- **`handle.meta`** — this route's `<title>` and `<meta>`, derived from its own
  data.

A loader that throws a `Response` stops the static build rather than writing a
broken page:

```tsx
if (!post) throw new Response("Not Found", { status: 404 });
```

Finding that at build time costs a failed build. Finding it in production costs
somebody reporting a 404 you shipped.

`dist/static/404.html` is rendered too, through the same app — most static hosts
serve it for anything they cannot find, which is what keeps a missing page
looking like the rest of the site instead of like the host's default.

## The development loop

```sh
npm run dev
```

Builds the browser bundle, serves `dist/`, and on every save rebuilds and
patches the change into the page you have open. A build that fails leaves the
last good one being served.

**The prerender does not run in development**, deliberately: rendering every
route on every keystroke buys nothing, because what you are looking at is the
same components with the same loaders. The pages that come out of
`npm run build` are the pages you were just looking at.

**Edit a component and it re-renders with its state intact** — a counter keeps
counting, a form keeps what you typed. That is React Fast Refresh, on by default
(`npm run dev -- --no-hot` turns it off).

A change nothing can absorb still reloads the page: editing a module that no
component boundary covers, or the document itself. And any module can take part
directly, React or not:

```js
import.meta.hot.accept();                                    // replace me
import.meta.hot.keep("cache", () => new Map());              // survive replacement
addEventListener("x", fn, { signal: import.meta.hot.signal }); // torn down for me
```

## Permissions

There is nothing to grant. This project builds files; it does not run a server,
so it has no ports, no environment, and no filesystem of its own in production —
your host serves a directory.

The build itself runs under `esdev`, which is a development tool and holds the
capabilities a build needs (reading your sources, writing `dist/`). Nothing in
that reaches a deployment.

## Styling

`styles/app.css` is linked from `index.html`. esdev resolves its `@import`s at
build time into one hashed file, and follows `url()` so fonts and images travel
with the stylesheet. `npm run build` minifies; `esdev build` on its own does not,
if you want to read the output.

Nesting and `color-mix()` are written as-is and shipped as-is — they are
supported everywhere this targets, so nothing lowers them.

### CSS Modules

A file named `*.module.css` is **scoped to itself**. Import it and you get the
real class names:

```tsx
import styles from "./Callout.module.css";

<aside className={styles.box}>…</aside>
```

`.box` becomes `.box_330a4019`, so two components can both declare `.box` and
neither wins. `src/app/Callout.module.css` is the worked example.

- The name is derived from the file's **path**, so it is the same in the
  prerendered HTML and in the browser (hydration is clean) and the same on every
  machine building this commit. Editing the file does not rename its classes.
- `:global(.no-js) .box` opts a name out of scoping. The wrapper is removed on
  the way out.
- Every module's CSS is collected into **one stylesheet, linked from the
  document** — not injected by script, so there is no flash of unstyled content.

**`composes`** reuses a class without repeating its rules:

```css
.button {
  composes: rounded from "./base.module.css";
  color: white;
}
```

`styles.button` then becomes two names — `"button_a1b2 rounded_e5f6"` — and the
element carries both. Three forms: `composes: a b` (this file),
`composes: a from "./x.module.css"`, and `composes: a from global`. It is
transitive, so composing a class that itself composes gets you the whole chain.

Order in the class list does not decide the cascade — specificity and position
in the stylesheet do, as always. So compose things that do not overlap.

### Importing a plain stylesheet

```js
import "some-package/dist/style.css";
```

A `.css` that is *not* `.module.css` is emitted unscoped. That is what
third-party CSS needs: a library's own JavaScript emits its class names as
hardcoded strings, so scoping them would rename half of a contract the library
has with itself.

For your own global styles prefer the `<link>` in `index.html` — it is fetched
without waiting for the bundle.

## Types for `runtime:`

`@opentf/esrun-types` is already a dev dependency and already named in
`tsconfig.json`, so `npm run typecheck` works on a fresh clone. If you ever need
to re-wire it — a new tsconfig, a moved project — `esdev --install-types` does
both halves again.

Types are for your editor and `npm run typecheck`; `esdev` erases them and never
checks them.

## Tests

```sh
npm test
```

`esdev test` runs each file unbundled. React ships CommonJS, which this runtime
does not load, so **a test that imports a component is refused** — component
testing needs a bundling step esdev does not have yet.

What that leaves is worth having anyway, and `src/http/head.ts` is factored for
it: choosing a page's title and escaping it are pure functions with no imports,
and a title is very often somebody else's string.
