import { serve } from "runtime:http";
import { env } from "runtime:process";

const port = Number(env.PORT ?? "8080");

serve({ port }, () => new Response("Hello from {{name}}\n"));
console.log(`listening on http://localhost:${port}`);
