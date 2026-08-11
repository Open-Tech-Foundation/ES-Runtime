// UDP round trips: 10 000 request/response exchanges over loopback, 64-byte
// payloads. The shape a DNS resolver, a game tick or an acknowledged telemetry
// packet actually has — send one datagram, wait for the answer, send the next —
// so what it measures is the per-datagram cost of a runtime's UDP API, latency
// included, with no pipelining to hide it behind.
//
// UDP is not a Web API, so every runtime is measured on its own surface, as the
// hashing and spawn rows already are: `runtime:net` `bind()` for esrun,
// `Bun.udpSocket`, `Deno.listenDatagram` (behind `--unstable-net`), and
// `node:dgram` for Node and LLRT.
//
// **The echo side is idiomatic per runtime, the client side always awaits.**
// Node and Bun deliver datagrams to a callback, so their echo handler is a
// callback; esrun and Deno hand back a promise, so theirs is a loop. The client
// then awaits its reply either way — that is the *program's* requirement, not
// one API's style, and adapting an event to it is part of what that API costs a
// request/response program.
//
// Both sockets live in this one process. A datagram cannot be lost with a single
// exchange in flight on loopback, so every runtime does exactly 10 000 round
// trips or the row fails — nothing can look fast by dropping work.
(async () => {
  const N = 10_000;
  const WARMUP = 1_000;
  const payload = new Uint8Array(64);
  crypto.getRandomValues(payload);

  // Each adapter returns { echoPort, roundTrip(payload), close() }.
  const setup = await (async () => {
    let bind = null;
    try {
      bind = (await import("runtime:net")).bind;
    } catch {}

    if (typeof bind === "function") {
      const echo = bind({ hostname: "127.0.0.1", port: 0 });
      const client = bind({ hostname: "127.0.0.1", port: 0 });
      const { port: echoPort } = await echo.addr;
      await client.connect({ hostname: "127.0.0.1", port: echoPort });
      (async () => {
        for (;;) {
          const d = await echo.receive();
          if (d === null) return;
          await echo.send(d.data, { hostname: d.address, port: d.port });
        }
      })();
      return {
        echoPort,
        async roundTrip(data) {
          await client.send(data);
          return (await client.receive()).data.length;
        },
        async close() {
          await client.close();
          await echo.close();
        },
      };
    }

    if (typeof Bun !== "undefined") {
      const echo = await Bun.udpSocket({
        hostname: "127.0.0.1",
        port: 0,
        socket: {
          data(socket, buf, port, addr) {
            socket.send(buf, port, addr);
          },
        },
      });
      let waiting = null;
      const client = await Bun.udpSocket({
        hostname: "127.0.0.1",
        port: 0,
        socket: {
          data(_socket, buf) {
            const resolve = waiting;
            waiting = null;
            resolve(buf.length);
          },
        },
      });
      return {
        echoPort: echo.port,
        roundTrip(data) {
          const reply = new Promise((resolve) => (waiting = resolve));
          client.send(data, echo.port, "127.0.0.1");
          return reply;
        },
        close() {
          client.close();
          echo.close();
        },
      };
    }

    if (typeof Deno !== "undefined" && typeof Deno.listenDatagram === "function") {
      const echo = Deno.listenDatagram({ hostname: "127.0.0.1", port: 0, transport: "udp" });
      const client = Deno.listenDatagram({ hostname: "127.0.0.1", port: 0, transport: "udp" });
      (async () => {
        for (;;) {
          try {
            const [data, from] = await echo.receive();
            await echo.send(data, from);
          } catch {
            return; // closed
          }
        }
      })();
      const to = echo.addr;
      return {
        echoPort: to.port,
        async roundTrip(data) {
          await client.send(data, to);
          const [reply] = await client.receive();
          return reply.length;
        },
        close() {
          client.close();
          echo.close();
        },
      };
    }

    const dgram = await import("node:dgram");
    const echo = dgram.createSocket("udp4");
    const client = dgram.createSocket("udp4");
    echo.on("message", (msg, rinfo) => echo.send(msg, rinfo.port, rinfo.address));
    let waiting = null;
    client.on("message", (msg) => {
      const resolve = waiting;
      waiting = null;
      resolve(msg.length);
    });
    await new Promise((r) => echo.bind(0, "127.0.0.1", r));
    await new Promise((r) => client.bind(0, "127.0.0.1", r));
    const echoPort = echo.address().port;
    return {
      echoPort,
      roundTrip(data) {
        const reply = new Promise((resolve) => (waiting = resolve));
        client.send(data, echoPort, "127.0.0.1");
        return reply;
      },
      close() {
        client.close();
        echo.close();
      },
    };
  })();

  const run = async (n) => {
    let bytes = 0;
    for (let i = 0; i < n; i++) bytes += await setup.roundTrip(payload);
    return bytes;
  };

  await run(WARMUP); // untimed warmup (JIT + socket buffers settled)
  const t0 = performance.now();
  const bytes = await run(N);
  const t1 = performance.now();
  await setup.close();
  if (bytes !== N * payload.length) throw new Error("lost datagrams: " + bytes);
  console.log("RESULT_MS=" + (t1 - t0).toFixed(2));
})();
