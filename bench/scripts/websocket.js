// WebSocket benchmark: message ping-pong round-trips against a local echo
// server (started by run.sh; the workload is skipped if it isn't running). The
// only workload that exercises the WebSocket client seam end-to-end — the
// opening handshake, then per-message send + event dispatch. For esrun that is
// the `ws_send` op plus the receive-pump turning each inbound frame into a
// `MessageEvent` on the tick, so this isolates the push→pull bridge cost.
(async () => {
  const URL_ = "ws://127.0.0.1:18924/";
  const N = 20000;
  const PAYLOAD = "ping-" + "x".repeat(32);

  const open = (url) =>
    new Promise((resolve, reject) => {
      const ws = new WebSocket(url);
      ws.onopen = () => resolve(ws);
      ws.onerror = () => reject(new Error("ws connect failed"));
    });

  // One serial chain of n send→echo round-trips over a single socket.
  const roundtrips = (ws, n) =>
    new Promise((resolve, reject) => {
      let i = 0;
      let total = 0;
      ws.onmessage = (e) => {
        total += typeof e.data === "string" ? e.data.length : e.data.byteLength;
        if (++i >= n) {
          resolve(total);
          return;
        }
        ws.send(PAYLOAD);
      };
      ws.onerror = () => reject(new Error("ws error"));
      ws.send(PAYLOAD); // kick off the chain
    });

  const ws = await open(URL_);
  await roundtrips(ws, N / 10); // warmup
  const t0 = performance.now();
  const total = await roundtrips(ws, N);
  const t1 = performance.now();
  if (total === -1) console.log(total); // defeat dead-code elimination
  ws.close();
  console.log("RESULT_MS=" + (t1 - t0).toFixed(2));
})();
