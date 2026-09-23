import { serve } from "runtime:http";
import { env, unmask } from "runtime:process";
import { hello } from "./hello.ts";

const port = Number(unmask(env.PORT ?? "8080"));

const server = serve({ port }, () => Response.json(hello("world")));

console.log(`listening on http://localhost:${(await server.addr).port}`);
