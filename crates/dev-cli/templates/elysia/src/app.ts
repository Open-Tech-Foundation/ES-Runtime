import { Elysia, t } from "elysia";
import { hello } from "./hello.ts";

function page(name: string): string {
  return (
    `<!doctype html>\n<html lang="en">\n<head>\n<meta charset="utf-8" />\n` +
    `<meta name="viewport" content="width=device-width, initial-scale=1" />\n` +
    `<title>${name}</title>\n` +
    `<style>body{font-family:system-ui,sans-serif;max-width:40rem;margin:4rem auto;padding:0 1rem}</style>\n` +
    `</head>\n<body>\n<h1>${hello("world").message}</h1>\n` +
    `<p>Built with <a href="https://elysiajs.com">Elysia</a> on ES Runtime. ` +
    `Edit <code>src/app.ts</code> to make it yours.</p>\n` +
    `<ul>\n<li><a href="/api/hello">/api/hello</a> — the greeting as JSON</li>\n` +
    `<li><a href="/api/greet?name=you">/api/greet?name=you</a> — validated input</li>\n` +
    `<li><a href="https://elysiajs.com/documentation">Elysia docs</a></li>\n` +
    `</ul>\n</body>\n</html>\n`
  );
}

/**
 * The application: routes and validation. No server here — see server.ts.
 * `listen()` is never called: under esrun the app is a fetch handler and
 * `runtime:http` owns the socket.
 */
export function createApp() {
  return new Elysia()
    .get("/api/hello", () => hello("world"))
    .get("/api/greet", ({ query }) => hello(query.name), {
      query: t.Object({ name: t.String({ minLength: 1 }) }),
    })
    .get("/", ({ set }) => {
      set.headers["content-type"] = "text/html; charset=utf-8";
      return page("{{name}}");
    });
}
