// UDP send throughput: 50 000 datagrams of 512 bytes, fire-and-forget over
// loopback. The other half of the UDP story from `udp_echo` — StatsD, syslog and
// telemetry send without waiting for anything, so what matters is the cost of
// handing one datagram to the OS rather than the latency of a reply.
//
// Each runtime is measured on its own surface (see `udp_echo` for why), and each
// pays what its API charges for that hand-off: esrun, Deno and Node await it
// (Node's callback is promisified — its `send` reports completion that way),
// while Bun's `send` is synchronous and returns `false` when the buffer is full,
// so it waits for `drain` and retries, which is Bun's documented backpressure
// path. Neither shape is being called wrong; the difference *is* the API.
//
// The destination is a real socket in this process that is never read, so the
// kernel's receive buffer fills and later datagrams are dropped — deliberately.
// A receiver would make this a measurement of the receive path, which `udp_echo`
// covers. Nothing here can look fast by dropping work: every send is awaited or
// retried, so all 50 000 leave this process.
(async () => {
  const N = 50_000;
  const WARMUP = 5_000;
  const payload = new Uint8Array(512);
  crypto.getRandomValues(payload);

  // Each adapter returns { send(payload), close() }, where `send` resolves once
  // the runtime says the datagram has been handed over.
  const setup = await (async () => {
    let bind = null;
    try {
      bind = (await import("runtime:net")).bind;
    } catch {}

    if (typeof bind === "function") {
      const sink = bind({ hostname: "127.0.0.1", port: 0 });
      const sender = bind({ hostname: "127.0.0.1", port: 0 });
      const { port } = await sink.addr;
      await sender.connect({ hostname: "127.0.0.1", port });
      return {
        send: (data) => sender.send(data),
        async close() {
          await sender.close();
          await sink.close();
        },
      };
    }

    if (typeof Bun !== "undefined") {
      const sink = await Bun.udpSocket({ hostname: "127.0.0.1", port: 0 });
      let drained = null;
      const sender = await Bun.udpSocket({
        hostname: "127.0.0.1",
        port: 0,
        socket: {
          drain() {
            const resolve = drained;
            drained = null;
            if (resolve) resolve();
          },
        },
      });
      const send = (data) => {
        if (sender.send(data, sink.port, "127.0.0.1")) return;
        // The buffer is full: wait for `drain`, then place the datagram.
        return new Promise((resolve) => (drained = resolve)).then(() => send(data));
      };
      return {
        send,
        close() {
          sender.close();
          sink.close();
        },
      };
    }

    if (typeof Deno !== "undefined" && typeof Deno.listenDatagram === "function") {
      const sink = Deno.listenDatagram({ hostname: "127.0.0.1", port: 0, transport: "udp" });
      const sender = Deno.listenDatagram({ hostname: "127.0.0.1", port: 0, transport: "udp" });
      const to = sink.addr;
      return {
        send: (data) => sender.send(data, to),
        close() {
          sender.close();
          sink.close();
        },
      };
    }

    const dgram = await import("node:dgram");
    const sink = dgram.createSocket("udp4");
    const sender = dgram.createSocket("udp4");
    await new Promise((r) => sink.bind(0, "127.0.0.1", r));
    await new Promise((r) => sender.bind(0, "127.0.0.1", r));
    const port = sink.address().port;
    return {
      send: (data) => new Promise((resolve) => sender.send(data, port, "127.0.0.1", resolve)),
      close() {
        sender.close();
        sink.close();
      },
    };
  })();

  const run = async (n) => {
    for (let i = 0; i < n; i++) await setup.send(payload);
  };

  await run(WARMUP); // untimed warmup
  const t0 = performance.now();
  await run(N);
  const t1 = performance.now();
  await setup.close();
  console.log("RESULT_MS=" + (t1 - t0).toFixed(2));
})();
