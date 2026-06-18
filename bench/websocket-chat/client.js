// Bun-style WebSocket "chat" broadcast benchmark — the client load driver.
//
// Mirrors Bun's bench/websocket-server/chat workload: C clients join one room;
// every message a client sends is broadcast by the server to ALL connected
// clients (a chat fan-out). This driver runs a *bounded* closed loop — each
// client keeps exactly one message in flight (it sends the next only when its
// own previous message comes back), so load saturates at C without runaway
// amplification, and the steady-state rate is the round-trip throughput.
//
// Reports messages/sec: SENT = client `send` calls; RECV = total deliveries
// observed across all clients = the server's fan-out throughput (≈ SENT × C).
// Config is injected by the runner as `globalThis.__WS_*` (so the same file runs
// unmodified on esrun / node / bun / deno, which differ on process.env access).

const PORT = Number(globalThis.__WS_PORT || 4001);
const HOST = globalThis.__WS_HOST || "127.0.0.1";
const C = Number(globalThis.__WS_CLIENTS || 32);
const WARMUP_MS = Number(globalThis.__WS_WARMUP_MS || 1000);
const MEASURE_MS = Number(globalThis.__WS_MEASURE_MS || 3000);
const PAYLOAD = "x".repeat(16);

const nowMs =
  typeof performance !== "undefined" ? () => performance.now() : () => Date.now();

let sent = 0;
let received = 0;
let baseSent = 0;
let baseRecv = 0;
let t0 = 0;
let openCount = 0;
let done = false;
const sockets = new Array(C);

function start() {
  // Initial kick: every client sends its first (tagged) message.
  for (let i = 0; i < C; i++) {
    sockets[i].send(i + ":" + PAYLOAD);
    sent++;
  }
  setTimeout(() => {
    baseSent = sent;
    baseRecv = received;
    t0 = nowMs();
    setTimeout(finish, MEASURE_MS);
  }, WARMUP_MS);
}

function finish() {
  done = true; // ignore teardown 'error' events from here on
  const dt = (nowMs() - t0) / 1000;
  const sps = Math.round((sent - baseSent) / dt);
  const rps = Math.round((received - baseRecv) / dt);
  console.log("CLIENTS=" + C);
  console.log("MSG_SENT_PER_SEC=" + sps);
  console.log("MSG_RECV_PER_SEC=" + rps);
  for (const ws of sockets) {
    try {
      ws.close();
    } catch {}
  }
}

for (let i = 0; i < C; i++) {
  const tag = i + ":";
  const ws = new WebSocket(`ws://${HOST}:${PORT}/`);
  sockets[i] = ws;
  ws.onmessage = (e) => {
    received++;
    const data = e.data;
    // Only the originator advances the loop — one message in flight per client.
    if (typeof data === "string" && data.startsWith(tag)) {
      sent++;
      ws.send(tag + PAYLOAD);
    }
  };
  ws.onopen = () => {
    if (++openCount === C) start();
  };
  ws.onerror = () => {
    if (!done) console.log("MSG_SENT_PER_SEC=ERR");
  };
}
