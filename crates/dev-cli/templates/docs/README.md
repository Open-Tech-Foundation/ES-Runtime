# {{name}}

An OTF Web documentation site, scaffolded by `esdev create`. MDX docs with a
sidebar, search, and pre-rendered pages — plus an optional demo blog.

## Run it

```sh
pnpm install
pnpm run dev      # local development
pnpm run build    # pre-rendered static site in dist/
```

`otfw` is the OTF Web toolchain. It runs on the ES runtime (`esdev`), so there
is nothing else to install — no Bun, no Node toolchain.

## Start building

- `app/docs/` — documentation pages in MDX. `_meta.js` controls sidebar order.
- `app/blog/` — demo blog posts (remove the tree, the `blog` block in
  `otfw.config.js`, and the Blog nav entry if you only need docs).
- `otfw.config.js` — site title, nav, footer, and blog settings.
