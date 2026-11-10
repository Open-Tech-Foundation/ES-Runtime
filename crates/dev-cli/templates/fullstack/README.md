# {{name}}

An [AFW](https://afw.opentechf.org) fullstack app, rendered on the server per
request. Start with `app/page.jsx`. Server pieces live beside it:
`app/loader.js` feeds the page, `app/api/hello/` is an API route,
`app/todos/todos.server.js` holds server functions the todos page calls like
async functions, and `app/_middleware.js` wraps every request.

`server.js` hands them all to one generated handler (`@afw/server`), so a new
route, loader, server function or middleware needs no edit there.

```sh
{{pm}} install
{{pm}} run dev      # esdev start — builds both targets, runs the server
{{pm}} run build     # esdev build — dist/
{{pm}} run start     # serves dist/ (run build first)
{{pm}} run test      # endpoint, loader and server-function tests, no server needed
```
