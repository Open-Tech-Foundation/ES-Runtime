# {{name}}

An OTF Web fullstack app, scaffolded by `esdev create`. UI plus server code —
middleware, route loaders, API routes, and per-request rendering via
`otfw serve`.

## Run it

```sh
pnpm install
pnpm run dev      # local development
pnpm run build    # production bundle in dist/
pnpm run serve    # build, then serve it per request
```

`otfw` is the OTF Web toolchain. It runs on the ES runtime (`esdev`), so there
is nothing else to install — no Bun, no Node toolchain.

## Start building

- `app/page.jsx` — the page. `app/loader.js` feeds it server-side data.
- `app/_middleware.js` — runs before pages, loaders, and API routes.
- `app/api/hello/route.js` — a sample API route at `/api/hello`.
