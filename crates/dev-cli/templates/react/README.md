# {{name}}

A React app on the ES-Runtime: server-rendered, hydrated in the browser, and
prerenderable to static HTML — one project, one build.

```sh
<pm> install
<pm> run dev      # http://localhost:8080
```

## What is here

| | |
| --- | --- |
| `index.html` | The document. Your title, your meta tags — and the two tags that name the build's inputs |
| `src/app/` | Components and the route table |
| `src/server.tsx` | The server. **This is the file production runs** |
| `src/render.tsx` | One render, streamed — shared by the server and the static build |
| `src/prerender.tsx` | Writes every route to `dist/static/` |
| `esdev.json` | What this project builds, and what `esdev start` runs |

## Types for `runtime:`

Your editor will not know what `runtime:http` is until the definitions are
installed. One command adds them and wires up `tsconfig.json`:

```sh
esdev --install-types
```

Types are for your editor and `tsc --noEmit`; `esdev` erases them and never
checks them.

## The three ways to ship it

One `<pm> run build` produces all three. Pick the one you want.

| | Deploy | Run |
| --- | --- | --- |
| **Server-rendered** | `dist/` | `esrun --allow-read=./dist --allow-listen=8080 dist/server.js` |
| **Static** | `dist/static/` | any static host |
| **Single-page** | `dist/index.html` + `dist/assets/` | any static host, with a fallback to `index.html` |

The client entry hydrates what the server rendered, or renders from scratch when
it finds an empty root — which is what lets one bundle serve all three.

## The development loop

```sh
<pm> run dev
```

Builds, runs `src/server.tsx`, and on every save rebuilds, restarts and reloads
the page. A build that fails leaves the running server alone.

It is a full page load, not hot module replacement — component state does not
survive a save.

## Permissions

The server runs under exactly what `esdev.json` grants it, in development and in
production:

```
--allow-read=./dist --allow-listen=8080
```

No filesystem beyond `dist`, no network, no subprocesses, no environment. Add
what you need as you need it — `esdev --trace-permissions dist/server.js` prints
the line for what a run actually used.

## Things this template does not do

- **No CSS pipeline.** `styles.css` is linked from `index.html` and copied by
  the build; `import "./x.css"` from JavaScript is not supported.
- **No client-side router.** Navigation is `<a href>`, a full page load. The
  route table in `src/app/routes.ts` is three fields; a router is yours to add.
- **No component tests.** `esdev test` runs each file unbundled, and React ships
  CommonJS, so a test that imports a component is refused. `src/app/routes.test.ts`
  shows what does work.
