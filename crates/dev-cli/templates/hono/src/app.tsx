import { Hono } from "hono";
import { HTTPException } from "hono/http-exception";
import { validator } from "hono/validator";
import { hello } from "./hello.ts";

function Page({ name }: { name: string }) {
  return (
    <html lang="en">
      <head>
        <meta charset="utf-8" />
        <meta name="viewport" content="width=device-width, initial-scale=1" />
        <title>{name}</title>
        <style>{`body{font-family:system-ui,sans-serif;max-width:40rem;margin:4rem auto;padding:0 1rem}`}</style>
      </head>
      <body>
        <h1>{hello("world").message}</h1>
        <p>
          Built with <a href="https://hono.dev">Hono</a> on ES Runtime. Edit{" "}
          <code>src/app.tsx</code> to make it yours.
        </p>
        <ul>
          <li>
            <a href="/api/hello">/api/hello</a> — the greeting as JSON
          </li>
          <li>
            <a href="/api/greet?name=you">/api/greet?name=you</a> — validated input
          </li>
          <li>
            <a href="https://hono.dev/docs">Hono docs</a>
          </li>
        </ul>
      </body>
    </html>
  );
}

/** The application: routes, validation and the page. No server here — see server.ts. */
export function createApp(): Hono {
  const app = new Hono();

  app.get("/api/hello", (c) => c.json(hello("world")));

  app.get(
    "/api/greet",
    validator("query", (value) => {
      const name = value["name"];
      if (typeof name !== "string" || name === "") {
        throw new HTTPException(400, { message: "pass ?name=<someone>" });
      }
      return { name };
    }),
    (c) => c.json(hello(c.req.valid("query").name)),
  );

  app.get("/", (c) => c.html(<Page name="{{name}}" />));

  return app;
}
