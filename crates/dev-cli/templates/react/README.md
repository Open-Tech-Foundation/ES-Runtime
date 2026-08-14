# {{name}}

A React app on the ES Runtime: server-rendered, hydrated in the browser, and
prerenderable to static HTML — one project, one build.

```sh
npm install
npm run dev      # http://localhost:8080
```

Swap `npm` for `bun`, `pnpm` or `yarn` throughout; nothing here depends on which
you use.

## What is here

| | |
| --- | --- |
| `src/routes.tsx` | **The app in one file** — paths, loaders, components, page titles |
| `src/server.tsx` | **What production runs** |
| `src/render.tsx` | One render, streamed — shared by the server and the static build |
| `src/prerender.tsx` | Writes every route to `dist/static/` |
| `src/app/` | The components |
| `src/http/` | Content types, security headers, escaping — the parts worth testing |
| `src/data/` | Stands in for your database or API |
| `styles/app.css` | The stylesheet. `@import` works; esdev bundles it |
| `index.html` | The document. Your meta tags, and the two markers the server fills |
| `esdev.json` | What this project builds, what it runs, what it is allowed to do |

## Routing

`src/routes.tsx` is a [react-router](https://reactrouter.com) route table in
data mode. A route may carry:

- **`loader`** — runs before the component, on whichever side is rendering. Its
  result reaches the browser with the document, so a hydrating page does not
  fetch what the server already had.
- **`Component`** — `useLoaderData()` gives it that result, typed per route.
- **`ErrorBoundary`** — what renders when a loader or a component below throws.
  A URL matching nothing arrives here too, as a 404.
- **`handle.meta`** — this route's `<title>` and `<meta>`, derived from its own
  data.

Throw a `Response` from a loader to answer with a status:

```tsx
if (!post) throw new Response("Not Found", { status: 404 });
```

That reaches the browser as a real 404, with your `ErrorBoundary` rendered into
it — not a soft 404 that returns 200 and tells search engines the page exists.

## The three ways to ship it

One `npm run build` produces all three. Pick the one you want.

| | Deploy | Run |
| --- | --- | --- |
| **Server-rendered** | `dist/` | `esrun --allow-read=./dist --allow-env=PORT --allow-listen=8080 dist/server.js` |
| **Static** | `dist/static/` | any static host |
| **Single-page** | `dist/index.html` + `dist/assets/` | any static host, with a fallback to `index.html` |

The client entry hydrates what the server rendered, or renders from scratch when
it finds an empty root — which is what lets one bundle serve all three.

## Permissions

The server runs under exactly what `esdev.json` grants it, in development and in
production:

```
--allow-read=./dist --allow-env=PORT --allow-listen=8080
```

No filesystem beyond `dist`, no outbound network, no subprocesses, and one
environment variable. Add what you need as you need it —
`esdev --trace-permissions dist/server.js` prints the line for what a run
actually used, and it is usually shorter than what you asked for.

There is no permissive development mode. A grant that is only added for
production is a grant nobody has tested.

## What the server does that a starter usually doesn't

- **A Content-Security-Policy with a per-response nonce.** The one inline script
  — react-router's hydration data — is allowed by nonce rather than by
  `'unsafe-inline'`, so an injected script still does not run.
- **`SIGTERM` drains rather than drops.** `server.stop()` closes the listener
  and lets requests in flight finish, which is what a rolling deploy needs.
- **Immutable caching, correctly.** Hashed filenames are cached for a year; a
  `npm run dev` build is not, because it reuses filenames.
- **A `/healthz` that touches nothing.** A health check that renders a page
  reports on the renderer, and one that queries a database takes the instance
  down when the database blinks.
- **One JSON log line per request**, which is what a log collector can read.

## Styling

`styles/app.css` is linked from `index.html`. esdev resolves its `@import`s at
build time into one hashed file, and follows `url()` so fonts and images travel
with the stylesheet. `npm run build -- --minify` drops comments and collapses
whitespace.

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

- The name is derived from the file's **path**, so it is the same on the server
  and in the browser (SSR hydrates cleanly) and the same on every machine
  building this commit. Editing the file does not rename its classes.
- `:global(.no-js) .box` opts a name out of scoping. The wrapper is removed on
  the way out.
- Every module's CSS is collected into **one stylesheet, linked from the
  document** — not injected by script, so there is no flash of unstyled content
  and no need for `style-src 'unsafe-inline'`.
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

Your editor will not know what `runtime:http` is until the definitions are
installed. One command adds them and wires up `tsconfig.json`:

```sh
esdev --install-types
```

Types are for your editor and `npm run typecheck`; `esdev` erases them and never
checks them.

## Tests

```sh
npm test
```

`esdev test` runs each file unbundled. React ships CommonJS, which this runtime
does not load, so **a test that imports a component is refused** — component
testing needs a bundling step esdev does not have yet.

What that leaves is worth having anyway, and `src/http/` is factored for it: the
content-type table, the asset-name check that stops path traversal, the HTML
escaping, and the CSP all sit behind pure functions with no imports.

## The development loop

```sh
npm run dev
```

Builds, runs `src/server.tsx`, and on every save rebuilds, restarts and reloads
the page. A build that fails leaves the running server alone.

It is a full page load, not hot module replacement — component state does not
survive a save. Router state does, because it is in the URL.
