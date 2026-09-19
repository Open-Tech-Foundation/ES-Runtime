# {{name}}

An OTF Web single-page app, scaffolded by `esdev create`. The UI runs entirely
in the browser — static deploy, no server files, API routes, or database.

## Run it

```sh
pnpm install
pnpm run dev      # local development
pnpm run build    # client bundle in dist/
pnpm run build:ssg  # pre-rendered static HTML
```

`otfw` is the OTF Web toolchain. It runs on the ES runtime (`esdev`), so there
is nothing else to install — no Bun, no Node toolchain.

## Start building

Edit `app/page.jsx`. Pages, layouts, and styles live under `app/`; the toolchain
mounts the app into `index.html`.
