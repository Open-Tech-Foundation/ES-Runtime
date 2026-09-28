# {{name}}

React on [ES Runtime](https://esrun.opentechf.org), rendered by a server of its own.
Start with `src/App.tsx` — `src/server.tsx` renders it per request.

```sh
{{pm}} install
{{pm}} run dev     # http://localhost:8080
{{pm}} run test
{{pm}} run build   # dist/server.js and the page it serves
{{pm}} run start   # runs the build with only the permissions it needs
```
