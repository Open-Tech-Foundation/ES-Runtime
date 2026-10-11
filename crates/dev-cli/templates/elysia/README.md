# {{name}}

An HTTP app on [Elysia](https://elysiajs.com), running on
[ES Runtime](https://esrun.opentechf.org). Routes and validation live in
`src/app.ts`, the server in `src/server.ts`, the greeting in
`src/hello.ts`.

The app never calls Elysia's `listen()` — that serves through Bun, which
is not here. It exposes a fetch handler instead, and `runtime:http` owns
the socket.

```sh
{{pm}} install
{{pm}} run dev     # http://localhost:8080
{{pm}} run test    # routes tested through app.handle, no server needed
{{pm}} run build   # dist/server.js
{{pm}} run start   # runs the build with only the permissions it needs
```
