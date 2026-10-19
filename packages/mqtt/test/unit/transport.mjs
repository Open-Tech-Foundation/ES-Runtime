// The real socket transport, and specifically what `close` does with a write
// the peer has stopped reading. The fake transport in `broker.mjs` closes the
// pipe outright, so it never showed this: on a socket the writer's close waits
// for the pending write, and the socket close that would interrupt it is only
// reached after — so the failure path has to be able to abort instead.
import { exit } from "runtime:process";
import { NetTransport } from "../../dist/transport.js";
import { ok, report } from "./assert.mjs";

const tick = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

// A socket whose writable never lets a write finish on its own — a peer that has
// stopped reading, which is exactly the state that makes `writer.close()` wait
// forever. `close()` records the call and, like the real socket's close, tears
// the connection down: that is what settles the in-flight write, so a close that
// reaches the socket at all does not leave the write dangling.
function stuckSocket() {
  const state = { writes: 0, closed: false };
  let settle = null;
  const writable = new WritableStream({
    write() {
      state.writes++;
      return new Promise((resolve, reject) => {
        settle = { resolve, reject };
      });
    },
    close() {},
    abort() {},
  });
  const readable = new ReadableStream({
    start(controller) {
      controller.close();
    },
  });
  return {
    state,
    socket: {
      readable,
      writable,
      async close() {
        state.closed = true;
        if (settle !== null) settle.reject(new Error("the socket was closed"));
      },
    },
    release() {
      if (settle !== null) settle.resolve();
    },
  };
}

// --- a graceful close has to wait for a pending write ----------------------

{
  const { socket, state, release } = stuckSocket();
  const transport = new NetTransport(socket);
  const write = transport.write(new Uint8Array([1])).catch(() => {});
  await tick(0);
  ok(state.writes === 1, "the write reached the socket");

  let finished = false;
  const closing = transport.close().then(() => {
    finished = true;
  });
  await tick(50);
  ok(finished === false, "a graceful close waits for the pending write");
  ok(state.closed === false, "and does not close the socket while the write is stuck");

  release();
  await closing;
  await write;
  ok(finished === true, "and finishes once the write completes");
  ok(state.closed === true, "then closes the socket");
}

// --- an aborted close tears the socket down regardless ---------------------

{
  const { socket, state } = stuckSocket();
  const transport = new NetTransport(socket);
  const write = transport.write(new Uint8Array([2])).catch(() => {});
  await tick(0);

  const outcome = await Promise.race([
    transport.close({ abort: true }).then(() => "closed"),
    tick(200).then(() => "hung"),
  ]);
  ok(outcome === "closed", "an aborted close does not wait for the pending write");
  ok(state.closed === true, "the socket is closed even though the write never finished");
  // The teardown settles the in-flight write rather than leaving it dangling.
  const settled = await Promise.race([
    write.then(() => "settled"),
    tick(200).then(() => "pending"),
  ]);
  ok(settled === "settled", "the interrupted write settles");
}

// --- a graceful close with nothing pending still closes the socket ---------

{
  const { socket, state } = stuckSocket();
  const transport = new NetTransport(socket);
  await transport.close();
  ok(state.closed === true, "a graceful close with nothing pending closes the socket");
  ok((await transport.read()) === null, "and a read after close is end of stream");
}

exit(report("transport") > 0 ? 1 : 0);
