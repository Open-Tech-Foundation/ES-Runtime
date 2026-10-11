import { serve } from "runtime:http";
import { env, unmask } from "runtime:process";
import { createApp } from "./app.ts";

const port = Number(unmask(env.PORT ?? "8080"));

const app = createApp();
const server = serve({ port }, (request) => app.fetch(request));

console.log(`listening on http://localhost:${(await server.addr).port}`);
