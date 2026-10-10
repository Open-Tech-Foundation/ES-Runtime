# {{name}}

An HTTP app on [Hono](https://hono.dev), running on
[ES Runtime](https://esrun.opentechf.org). Routes and validation live in
`src/app.tsx`, the server in `src/server.ts`, the greeting in
`src/hello.ts`.

```sh
{{pm}} install
{{pm}} run dev     # http://localhost:8080
{{pm}} run test    # routes tested through app.request, no server needed
{{pm}} run build   # dist/server.js
{{pm}} run start   # runs the build with only the permissions it needs
```
