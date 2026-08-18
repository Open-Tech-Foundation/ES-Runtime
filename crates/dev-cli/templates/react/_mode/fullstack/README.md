# {{name}}

React and react-router on the [ES Runtime](https://esrun.opentechf.org),
rendered per request by a server you own.

```sh
npm install
npm run dev       # http://localhost:8080
```

Swap `npm` for `bun`, `pnpm` or `yarn`; nothing here depends on which you use.

## What is here

| | |
| --- | --- |
| `src/routes.tsx` | The route table: paths, loaders, components, page titles |
| `src/app/Home.tsx` | **Start here.** The one page this app has |
| `src/app/Layout.tsx` | The frame every page renders inside |
| `src/app/ErrorPage.tsx` | What a 404 and a thrown error render |
| `src/server.tsx` | **What production runs.** A `Request` in, a `Response` out |
| `index.html` | The document. Its `<script>` and `<link>` are the build's inputs |
| `src/render.tsx` | One render, used by the server and the browser both |

## Commands

| | |
| --- | --- |
| `npm run dev` | Build, run the server, rebuild and restart on save |
| `npm test` | `esdev test` — every `*.test.ts` |
| `npm run build` | Server and browser bundles into `dist/` |
| `npm start` | Run `dist/` with `esrun`, under the grants below |
| `npm run typecheck` | `tsc --noEmit`. esdev erases types and never checks them |

## What it is allowed to do

`esdev.json` names it, and `npm run dev` runs the server under exactly the same
line production does — a grant only added for production is a grant nobody has
tested:

```sh
--allow-read=./dist --allow-env=PORT --allow-listen=8080 \
--allow-signals=SIGTERM,SIGINT
```

No filesystem beyond `dist`, no outbound network, no subprocesses, and one
environment variable. The server already answers `HEAD`, sets a
Content-Security-Policy with a per-response nonce, caches hashed assets
immutably, drains on `SIGTERM`, and logs one JSON line per request.

## Docs

[esrun.opentechf.org/docs](https://esrun.opentechf.org/docs) ·
[API](https://esrun.opentechf.org/api) ·
[GitHub](https://github.com/Open-Tech-Foundation/ES-Runtime)

Part of the [Open Tech Foundation](https://github.com/Open-Tech-Foundation)
ecosystem.
