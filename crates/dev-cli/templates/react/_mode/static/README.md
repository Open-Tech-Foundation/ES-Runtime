# {{name}}

React and react-router on the [ES Runtime](https://esrun.opentechf.org),
prerendered — **nothing runs in production, so this project grants nothing.**

```sh
npm install
npm run dev       # http://localhost:5173
```

Swap `npm` for `bun`, `pnpm` or `yarn`; nothing here depends on which you use.

## What is here

| | |
| --- | --- |
| `src/routes.tsx` | The route table: paths, loaders, components, page titles |
| `src/app/Home.tsx` | **Start here.** The one page this app has |
| `src/app/Layout.tsx` | The frame every page renders inside |
| `src/app/ErrorPage.tsx` | What a 404 and a thrown error render |
| `src/paths.ts` | Which routes the build writes as files |
| `index.html` | The document. Its `<script>` and `<link>` are the build's inputs |
| `src/render.tsx` | One render, used by the prerender step and the browser both |

## Commands

| | |
| --- | --- |
| `npm run dev` | The dev server, rebuilding on save |
| `npm test` | `esdev test` — every `*.test.ts` |
| `npm run build` | Every route in `src/paths.ts` prerendered to `dist/static/` |
| `npm run build:spa` | One shell instead, routed in the browser |
| `npm run typecheck` | `tsc --noEmit`. esdev erases types and never checks them |

Both builds come out of the same routes and components with no file edited, so
which one a deploy wants is a decision that can change later. Deploy the output
to any static host.

Want a server of its own — rendered per request, under named capabilities?
`esdev create <name> --template=react --mode=fullstack`.

## Docs

[esrun.opentechf.org/docs](https://esrun.opentechf.org/docs) ·
[API](https://esrun.opentechf.org/api) ·
[GitHub](https://github.com/Open-Tech-Foundation/ES-Runtime)

Part of the [Open Tech Foundation](https://github.com/Open-Tech-Foundation)
ecosystem.
