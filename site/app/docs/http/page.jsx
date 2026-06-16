import DocsShell from "../../../components/DocsShell.jsx";
import CodeBlock from "../../../components/CodeBlock.jsx";

const HTTP = `import { serve } from "runtime:http";

// serve(options, handler) — the handler takes a Request, returns a Response.
serve({ port: 8080 }, (req) => {
  return new Response("Hello from esrun!");
});

console.log("Server listening on port 8080");`;

export default function HttpDoc() {
  return (
    <DocsShell active="/docs/http">
      <p className="text-sm font-medium text-brand-600">Guides</p>
      <h1 className="mt-2 text-4xl font-bold tracking-tight text-zinc-900">
        HTTP Server
      </h1>
      <p className="mt-4 text-lg leading-relaxed text-zinc-600">
        The <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">runtime:http</code> module provides a fast, built-in HTTP server using standard web <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">Request</code> and <code className="rounded bg-zinc-100 px-1.5 py-0.5 text-[13px]">Response</code> objects.
      </p>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Usage</h2>
      <div className="mt-4">
        <CodeBlock code={HTTP} title="server.js" lang="js" />
      </div>

      <h2 className="mt-12 text-xl font-semibold text-zinc-900">Usage with Hono</h2>
      <p className="mt-3 text-zinc-600">
        Because esrun uses standard Web APIs, popular frameworks like Hono work out of the box.
      </p>
      <div className="mt-4">
        <CodeBlock code={`import { serve } from "runtime:http";
import { Hono } from "hono";

const app = new Hono();
app.get("/", (c) => c.text("Hello from Hono on ESRun!"));

serve({ port: 8080 }, app.fetch);

console.log("Hono listening on port 8080");`} title="hono.js" lang="js" />
      </div>
    </DocsShell>
  );
}
