# {{name}}

An [OTF Web](https://web.opentechf.org) fullstack app. Start with `app/page.jsx`.
Server pieces live beside it: `app/loader.js` feeds the page,
`app/api/hello/` is an API route, and `app/_middleware.js` wraps every
request. `server.js` wires it all to `runtime:http` (`bootstrap.js` shims
the DOM globals the shared runtime needs at module scope).

```sh
{{pm}} install
{{pm}} run dev      # esdev start — builds both targets, runs the server
{{pm}} run build     # esdev build — dist/
{{pm}} run start     # serves dist/ (run build first)
{{pm}} run test      # endpoint + loader tests, no server needed
```
