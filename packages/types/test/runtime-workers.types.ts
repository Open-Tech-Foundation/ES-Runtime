// A type test for `runtime:workers`: shard settings and hibernatable sockets.
// Compiled by `tsc -p .`, never run.

import type { WebSocketConnection } from "runtime:websocket";
import {
  configure,
  DurableErrorCode,
  type DurableSocket,
  DurableWorker,
  host,
  Isolate,
} from "runtime:workers";

const set = configure({
  shards: "auto",
  module: new URL("./classes.js", import.meta.url),
  permissions: ["net"],
});
const count: number = set.shards;
const module: string | null = set.module;
configure({ shards: 2, module: "file:///app/classes.js" });
configure({ shards: 0, module: null });

const lost: "ERR_DURABLE_SHARD_LOST" = DurableErrorCode.ShardLost;

// @ts-expect-error — a count, or "auto".
configure({ shards: "many" });

void count;
void module;
void lost;

// Hibernatable WebSockets (D133): a class takes a socket, and a caller passes
// the connection itself.

class Room extends DurableWorker {
  join(ws: DurableSocket, name: string): number {
    this.ctx.acceptWebSocket(ws, [name]);
    ws.serializeAttachment({ name });
    return this.ctx.getWebSockets(name).length;
  }
  override webSocketMessage(ws: DurableSocket, message: string | ArrayBuffer): void {
    const who = ws.deserializeAttachment<{ name: string }>()?.name;
    for (const s of this.ctx.getWebSockets()) s.send(`${who}: ${String(message)}`);
  }
}

declare const conn: WebSocketConnection;
const joined: Promise<number> = Room.get("lobby").join(conn, "ana");
// @ts-expect-error — handlers are the runtime's to call, not a caller's.
Room.get("lobby").webSocketMessage;
void joined;

// Isolates (D227): computation only, reached through exports and `host`.

const rule = new Isolate(
  { source: "export const f = (x) => x * 2;", name: "double" },
  {
    host: { log: (line: string) => console.log(line) },
    memory: 16,
    cpuTime: 50,
  },
);
const doubled: Promise<unknown> = rule.exports.then(({ f }) => f(21));
rule.postMessage({ any: "value" });
rule.terminate();
// @ts-expect-error — an isolate takes no permissions.
new Isolate(new URL("./p.js", import.meta.url), { permissions: ["net"] });
const warm: Promise<number> = Isolate.prepare(8);
void warm;
const viaHost: Promise<unknown> | undefined = host?.config("shout");
void doubled;
void viaHost;
