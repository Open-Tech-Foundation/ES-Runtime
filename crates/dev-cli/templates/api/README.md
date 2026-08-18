# {{name}}

A JSON API on the [ES Runtime](https://esrun.opentechf.org). **Nothing it ships
depends on**, and it starts with the permissions it needs and no others.

```sh
npm install       # TypeScript and the runtime's types, both dev-only
npm run dev       # http://localhost:8080
curl localhost:8080/
```

Swap `npm` for `bun`, `pnpm` or `yarn`; nothing here depends on which you use.

## What is here

| | |
| --- | --- |
| `src/server.ts` | **Start here.** The route table, and what production runs |
| `src/router.ts` | `URLPattern` matching — a table and a loop, no dependency |
| `src/http.ts` | JSON responses, and the one error type that becomes one |

## Commands

| | |
| --- | --- |
| `npm run dev` | Build, run, rebuild and restart on save |
| `npm test` | `esdev test` — every `*.test.ts` |
| `npm run build` | → `dist/server.js` |
| `npm start` | Run `dist/` with `esrun`, under the grants below |
| `npm run typecheck` | `tsc --noEmit`. esdev erases types and never checks them |

## What it is allowed to do

`esdev.json` names it, and `npm run dev` runs the server under exactly the same
line production does — a grant only added for production is a grant nobody has
tested:

```sh
--allow-listen=8080 --allow-env=PORT --allow-signals=SIGTERM,SIGINT
```

No filesystem at all — not even read — no outbound network, no subprocesses,
and one environment variable. If this process is ever made to run somebody
else's code, that is the whole of what it can reach.

`esdev --trace-permissions dist/server.js` prints what a run actually used.

## Docs

[esrun.opentechf.org/docs](https://esrun.opentechf.org/docs) ·
[API](https://esrun.opentechf.org/api) ·
[GitHub](https://github.com/Open-Tech-Foundation/ES-Runtime)

Part of the [Open Tech Foundation](https://github.com/Open-Tech-Foundation)
ecosystem.
