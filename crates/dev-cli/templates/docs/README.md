# {{name}}

An [OTF Web](https://web.opentechf.org) documentation site. Pages are MDX files under `app/docs/`.
New pages are folders with a `page.mdx`; order the sidebar in `app/docs/_meta.js`.

```sh
{{pm}} install
{{pm}} run dev        # esdev start — the dev loop, no prerender
{{pm}} run build      # esdev build — prerenders to dist/, indexes search
{{pm}} run preview    # serves dist/ the way it will be served
```
