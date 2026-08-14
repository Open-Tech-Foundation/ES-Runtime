# {{name}}

A JSON API on the ES Runtime. **No dependencies** — not one.

```sh
npm install       # nothing to install, but it writes the lockfile
npm run dev       # http://localhost:8080
```

Swap `npm` for `bun`, `pnpm` or `yarn`; nothing here depends on which you use.

```sh
curl localhost:8080/tasks
curl -X POST localhost:8080/tasks -H 'content-type: application/json' -d '{"title":"Write it down"}'
curl -X DELETE localhost:8080/tasks/<id>
```

## What is here

| | |
| --- | --- |
| `src/server.ts` | **What production runs** — the routes, the log line, the shutdown |
| `src/router.ts` | Matching a request to a handler, on `URLPattern` |
| `src/http.ts` | Responses, and the one error type that becomes one |
| `src/tasks.ts` | The resource, and the `Map` standing in for your database |

## Zero dependencies is the point

`URLPattern`, `Request`, `Response`, `URL`, `crypto.randomUUID()` — every one is
a web standard this runtime already has. A router is a table and a loop.

That is not minimalism for its own sake. Every dependency in a server is code
you did not read running with everything you were granted, and this template
exists partly to show how little you need.

## What it is allowed to do

```
--allow-listen=8080 --allow-env=PORT --allow-signals=SIGTERM,SIGINT
```

**No filesystem at all** — not even read. No outbound network, no subprocesses,
no environment beyond one variable. If this process is ever made to run somebody
else's code, that is the whole of what it can reach.

It runs under exactly that in development too — there is no permissive
development mode. A grant that is only added for production is a grant nobody
has tested. `esdev --trace-permissions dist/server.js` prints the line for what
a run actually used.

## Errors are thrown, not returned

```ts
if (!task) throw HttpError.notFound(`No task ${id}`);
```

A handler that threads an error value back through every call ends up checking
for it more often than it does anything else. Throwing lets the failure travel
to the one place that turns it into a response.

**Only an `HttpError` carries a message to the client.** Anything else is a bug,
and a bug's message names hostnames, paths and sometimes the data itself — so it
gets a flat 500 and the detail goes to the log.

| Thrown | Response |
| --- | --- |
| `HttpError.notFound(…)` | 404 with your message |
| `HttpError.badRequest(…)` | 400 |
| `HttpError.invalid({ field: … })` | 422 with the fields that were wrong |
| anything else | 500 `Internal Server Error`, logged |

## Tests cover everything

```sh
npm test
```

There is no React here, so nothing reaches CommonJS and **`esdev test` can run
every module**: the router, the error mapping, the JSON body reading, the
validation. 20 tests, no test framework, no mocks.

## Status codes it gets right

- A path that exists but does not answer your method is **405 with an `Allow`
  header**, not a 404. A client told only "no" learns nothing.
- `HEAD` is answered by the `GET` route. A second implementation is a second
  thing that can disagree with the first.
- A created resource returns **201 with a `Location`**.
- A delete returns **204** and no body.

## Deploying it

```sh
npm run build
esrun --allow-listen=8080 --allow-env=PORT --allow-signals=SIGTERM,SIGINT dist/server.js
```

One file. `SIGTERM` closes the listener and lets requests in flight finish
before the process exits, which is what a rolling deploy needs.

## Replacing the store

`src/tasks.ts` holds a `Map`. Swap it for `runtime:db`, or a `fetch` to a
service, and no handler changes — but **add the capability it needs to
`esdev.json`**, or the first query fails with the permission it was denied.
