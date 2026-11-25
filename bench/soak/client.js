// The client half of the HTTP soak, run beside the load generator: fetch with
// request and response bodies, and WebSocket connections opened, used and
// closed, each in several concurrent loops. A wrong answer exits non-zero, so a
// soak that "passes" by serving garbage fails instead.
import { env, exit } from "runtime:process";

const BASE = `http://127.0.0.1:${env.SOAK_PORT}`;
// TLS, where ALPN picks HTTP/2: a stream per request on one connection.
const SECURE = `https://localhost:${Number(env.SOAK_PORT) + 2}`;
const WS = `ws://127.0.0.1:${Number(env.SOAK_PORT) + 1}`;
let done = 0;

function fail(message) {
  console.error(`soak client: ${message}`);
  exit(1);
}

async function fetchLoop(base) {
  for (;;) {
    const body = { n: done, pad: "x".repeat(1024) };
    const echoed = await (await fetch(`${base}/echo`, { method: "POST", body: JSON.stringify(body) })).json();
    if (echoed.n !== body.n) fail(`echo returned ${echoed.n} for ${body.n}`);
    const streamed = await (await fetch(`${base}/stream`)).arrayBuffer();
    if (streamed.byteLength !== 8 * 16 * 1024) fail(`stream returned ${streamed.byteLength} bytes`);
    done++;
  }
}

async function wsLoop() {
  for (;;) {
    const socket = new WebSocket(WS);
    await new Promise((resolve, reject) => {
      socket.onopen = resolve;
      socket.onerror = () => reject(new Error("websocket failed to open"));
    });
    for (let i = 0; i < 10; i++) {
      const reply = new Promise((resolve) => (socket.onmessage = (e) => resolve(e.data)));
      socket.send(`m${i}`);
      if ((await reply) !== `m${i}`) fail("websocket echoed the wrong message");
    }
    await new Promise((resolve) => {
      socket.onclose = resolve;
      socket.close();
    });
    done++;
  }
}

setInterval(() => console.log(`progress ${done}`), 10_000);
await Promise.all([
  fetchLoop(BASE),
  fetchLoop(BASE),
  fetchLoop(SECURE),
  fetchLoop(SECURE),
  wsLoop(),
  wsLoop(),
  wsLoop(),
  wsLoop(),
]).catch(
  (e) => fail(e.message),
);
