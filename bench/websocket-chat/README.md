# WebSocket chat broadcast benchmark

A Bun-style WebSocket **chat** workload: `C` clients join one room, and the
server broadcasts every message it receives to *all* connected clients (the
fan-out). Reports **messages/sec** — `SENT` (client `send` calls) and `RECV`
(total deliveries = the server's fan-out throughput = `SENT × C`).

The client driver runs a **bounded closed loop**: each client keeps exactly one
message in flight (it sends the next only when its own previous message returns),
so load saturates at `C` without runaway amplification, and the steady-state rate
is the round-trip throughput. A fan-out ratio (`RECV / SENT`) equal to `C` means
*full delivery*; below `C` means the server is lagging (deliveries queued).

## Running

```sh
cargo build --release -p es-runtime-cli
bench/websocket-chat/run-chat.sh                 # C=32, both sweeps
WS_CLIENTS=128 REPS=3 bench/websocket-chat/run-chat.sh
```

Two sweeps:

- **Server sweep** — fixed Bun client driver, server ∈ {bun, deno, esrun}. The
  chat-server throughput. (esrun serves via `runtime:websocket` `serve()` +
  `broadcast()`.)
- **Client sweep** — fixed Bun server, client ∈ {esrun, bun, deno, node}. Each
  runtime's WebSocket *client* under the same broadcast load.

Knobs: `WS_CLIENTS` (32), `WS_WARMUP_MS` (1000), `WS_MEASURE_MS` (3000), `REPS`
(3, best-of), `ESRUN` (binary path). Each runtime serves with its built-in WS
server (Bun `Bun.serve`, Deno `Deno.upgradeWebSocket`, esrun
`runtime:websocket`); the client is each runtime's standard `WebSocket` global.

## Representative results

`RECV` messages/sec (server fan-out), one Linux x86-64 box; indicative, re-run
locally. All cells are full delivery (ratio = `C`).

```
# Server sweep (client driver = bun)
clients |       bun |      deno |     esrun
--------+-----------+-----------+-----------
     32 |   208,662 |   175,919 |   259,742
     64 |   211,235 |   176,871 |    91,833
    128 |   210,354 |   174,643 |    45,368

# Client sweep (server = bun), esrun RECV
     32 |   ~204,000   (on par with node/bun/deno)
    128 |   ~197,000
    256 |   ~172,000   (~12% behind node)
```

(bun 1.3, deno 2.8, node 24, esrun 0.4.)

## Interpretation

esrun serves through the **driven seam** (D4): hyper-style, each connection is a
spawned actor and the JS loop drives sends/receives over channels, capability-
secured — not a native pub/sub server. Two fan-out optimizations (DECISIONS D29)
make it competitive:

- **Batched `broadcast(connections, data)`** — one host op crossing + one payload
  marshal for the whole room (vs one `ws_send` per connection), the frame cloned
  O(1) (refcounted) and enqueued to every connection concurrently (`join_all`, so
  a slow peer can't head-of-line-block the rest). The op's completion is the
  backpressure that keeps delivery **full** instead of lagging.
- **Coalesced writes** — the per-connection writer `feed`s a burst of queued
  frames and `flush`es once (one socket write per drain, not per frame).

Result: at **C=32 esrun leads** (260k vs Bun's 209k) with full delivery. As `C`
grows the throughput falls off (92k at 64, 45k at 128) while Bun/Deno hold steady
— the remaining cost is the central `conns` map lock (each broadcast snapshots
all senders under it, ~O(C²) lock-held work per round) and the per-connection
actor/channel hop. Sharding that map (or caching the sender set per room) is the
next step for high-fan-out throughput; the **client** stays on par with native
across the board.
